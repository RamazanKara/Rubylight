//! Wake-on-LAN readiness: judging the NIC wake settings Windows reports, and
//! the plan that changes them (`docs/wake-and-audio-design.md`, A.3).
use super::{Adapter, Medium, format_mac, primary};
use rubylight_native_api::wake::{
    self as api, AdapterKind, Check, CheckState, FastStartup, ShutdownWake, Sleep, TargetState,
    check,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::IpAddr};

pub const MAGIC_PACKET: &str = "*WakeOnMagicPacket";
pub const WAKE_ON_PATTERN: &str = "*WakeOnPattern";
pub const MODERN_STANDBY_MAGIC_PACKET: &str = "*ModernStandbyWoLMagicPacket";
/// Realtek's wake from shutdown; each vendor defines its own meaning.
pub const SHUTDOWN_WAKE: &str = "S5WakeOnLan";

/// The wake settings of every physical adapter.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct NicReport {
    #[serde(default)]
    pub adapters: Vec<NicSettings>,
}

/// One physical adapter's wake settings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct NicSettings {
    /// `InterfaceGuid`, the key to [`Adapter::guid`].
    pub guid: String,
    /// The PnP device id, the key to the WMI instances.
    pub pnp: String,
    /// The localized adapter name, for logs and the fix's description only.
    pub name: String,
    /// `MSPower_DeviceWakeEnable.Enable`; `None` without a WMI instance.
    pub wake_enable: Option<bool>,
    /// `MSNdis_DeviceWakeOnMagicPacketOnly.EnableWakeOnMagicPacketOnly`.
    pub magic_packet_only: Option<bool>,
    /// The wake advanced properties the adapter has: `RegistryKeyword` to
    /// `RegistryValue`, never the localized display value.
    #[serde(deserialize_with = "keywords")]
    pub keywords: BTreeMap<String, String>,
    /// What could not be read, in the PC's language.
    pub errors: Vec<String>,
}

/// Keywords are case-insensitive in the registry; keep them in upper case.
fn keywords<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<String, String>, D::Error> {
    let map = Option::<BTreeMap<String, String>>::deserialize(d)?.unwrap_or_default();
    Ok(map
        .into_iter()
        .map(|(keyword, value)| (keyword.to_ascii_uppercase(), value.trim().to_owned()))
        .collect())
}

impl NicReport {
    pub fn parse(json: &str) -> anyhow::Result<Self> {
        let mut report: Self = serde_json::from_str(json.trim_start_matches('\u{feff}'))?;
        for adapter in &mut report.adapters {
            adapter.guid = adapter.guid.to_ascii_uppercase();
        }
        Ok(report)
    }
    pub fn get(&self, adapter: &Adapter) -> Option<&NicSettings> {
        self.adapters
            .iter()
            .find(|nic| !nic.guid.is_empty() && nic.guid.eq_ignore_ascii_case(&adapter.guid))
    }
}

impl NicSettings {
    pub fn keyword(&self, keyword: &str) -> Option<&str> {
        self.keywords
            .get(&keyword.to_ascii_uppercase())
            .map(String::as_str)
    }
    /// Wake on magic packet is on: what the MAC policy prefers.
    pub fn armed(&self) -> bool {
        self.wake_enable == Some(true) && self.keyword(MAGIC_PACKET) == Some("1")
    }
}

/// The power facts Windows reports (`GetPwrCapabilities`, `HiberbootEnabled`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemFacts {
    pub s3: bool,
    /// `AoAc`: Modern Standby.
    pub modern_standby: bool,
    /// `HiberFilePresent`.
    pub hibernate: bool,
    /// `HiberbootEnabled`; `None` when the value is missing.
    pub hiberboot: Option<bool>,
}

impl SystemFacts {
    /// Fast Startup is active only when hibernate is available.
    pub fn fast_startup_active(&self) -> bool {
        self.hiberboot == Some(true) && self.hibernate
    }
}

pub fn system(facts: Option<&SystemFacts>) -> api::System {
    let Some(facts) = facts else {
        return api::System {
            sleep: Sleep::Unknown,
            modern_standby: false,
            hibernate: false,
            fast_startup: FastStartup::Unknown,
            shutdown_wake: ShutdownWake::FirmwareUnknown,
        };
    };
    api::System {
        sleep: if facts.s3 { Sleep::S3 } else { Sleep::None },
        modern_standby: facts.modern_standby,
        hibernate: facts.hibernate,
        fast_startup: match facts.hiberboot {
            Some(true) if facts.hibernate => FastStartup::Active,
            Some(true) => FastStartup::Inactive,
            Some(false) => FastStartup::Off,
            None => FastStartup::Unknown,
        },
        // Firmware settings Windows can't read: never "ready".
        shutdown_wake: ShutdownWake::FirmwareUnknown,
    }
}

fn flag(value: Option<bool>, off: CheckState) -> CheckState {
    match value {
        Some(true) => CheckState::Ok,
        Some(false) => off,
        None => CheckState::Unknown,
    }
}

fn keyword_state(value: Option<&str>, off: CheckState) -> CheckState {
    match value {
        Some("0") => off,
        Some(_) => CheckState::Ok,
        None => CheckState::Unknown,
    }
}

/// The checks of one adapter, and its state: `NicSettings` is `Err` with the
/// reason when the reading failed or didn't report the adapter.
pub fn judge(
    adapter: &Adapter,
    settings: Result<&NicSettings, &str>,
    modern_standby: bool,
) -> (TargetState, Vec<Check>) {
    let check = |id: &str, state| Check {
        id: id.into(),
        state,
    };
    let Ok(nic) = settings else {
        let checks = [
            check::WAKE_ENABLED,
            check::MAGIC_PACKET,
            check::MAGIC_PACKET_ONLY,
        ]
        .map(|id| check(id, CheckState::Unknown))
        .into();
        let state = if adapter.medium == Medium::Wifi {
            TargetState::Experimental
        } else {
            TargetState::Unknown
        };
        return (state, checks);
    };
    let mut required = vec![
        check(
            check::WAKE_ENABLED,
            flag(nic.wake_enable, CheckState::Error),
        ),
        check(
            check::MAGIC_PACKET,
            keyword_state(nic.keyword(MAGIC_PACKET), CheckState::Error),
        ),
    ];
    if modern_standby && let Some(value) = nic.keyword(MODERN_STANDBY_MAGIC_PACKET) {
        required.push(check(
            check::MODERN_STANDBY_MAGIC_PACKET,
            keyword_state(Some(value), CheckState::Error),
        ));
    }
    let state = if adapter.medium == Medium::Wifi {
        TargetState::Experimental
    } else if required.iter().any(|c| c.state == CheckState::Error) {
        TargetState::NeedsSetup
    } else if required.iter().any(|c| c.state == CheckState::Unknown) {
        TargetState::Unknown
    } else {
        TargetState::Ready
    };
    let mut checks = required;
    checks.push(check(
        check::MAGIC_PACKET_ONLY,
        flag(nic.magic_packet_only, CheckState::Warning),
    ));
    // Information only: pattern wake lets network chatter wake the PC.
    if let Some(value) = nic.keyword(WAKE_ON_PATTERN) {
        let chatter = value != "0" && nic.magic_packet_only != Some(true);
        let state = if chatter {
            CheckState::Warning
        } else {
            CheckState::Ok
        };
        checks.push(check(check::WAKE_ON_PATTERN, state));
    }
    if let Some(value) = nic.keyword(SHUTDOWN_WAKE) {
        checks.push(check(
            check::SHUTDOWN_WAKE,
            keyword_state(Some(value), CheckState::Warning),
        ));
    }
    (state, checks)
}

/// Every physical adapter as a wake target, and the MAC `/serverinfo`
/// reports for a request that reached the PC at `local`. `report` is `Err`
/// with the reason when reading the NIC settings failed.
pub fn targets(
    adapters: &[Adapter],
    report: Result<&NicReport, &str>,
    modern_standby: bool,
    local: Option<IpAddr>,
) -> (Option<String>, Vec<api::Target>) {
    let armed = |adapter: &Adapter| {
        report
            .ok()
            .and_then(|report| report.get(adapter))
            .map(NicSettings::armed)
    };
    let chosen = local.and_then(|local| primary(adapters, local, armed));
    let targets = adapters
        .iter()
        .filter(|adapter| adapter.physical())
        .map(|adapter| {
            let settings = match report {
                Ok(report) => report
                    .get(adapter)
                    .ok_or("the NIC settings reading did not report this adapter"),
                Err(error) => Err(error),
            };
            let (state, checks) = judge(adapter, settings, modern_standby);
            api::Target {
                mac: format_mac(adapter.mac),
                kind: match adapter.medium {
                    Medium::Wifi => AdapterKind::Wifi,
                    _ => AdapterKind::Ethernet,
                },
                connected: adapter.connected,
                route: chosen.is_some_and(|c| c.route && std::ptr::eq(c.adapter, adapter)),
                broadcasts: adapter
                    .broadcasts()
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                state,
                checks,
            }
        })
        .collect();
    (chosen.map(|c| format_mac(c.adapter.mac)), targets)
}

/// The wake settings to change on one adapter.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterFix {
    pub guid: String,
    pub pnp: String,
    /// The localized name, for the description only.
    pub name: String,
    /// Advanced properties to set: keyword to `RegistryValue`. Changing one
    /// restarts the adapter.
    pub keywords: BTreeMap<String, String>,
    /// Turn on "Allow this device to wake the computer".
    pub wake_enable: bool,
    /// Turn on "Only allow a magic packet to wake the computer".
    pub magic_packet_only: bool,
}

/// What a fix changes, with consent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixPlan {
    pub adapters: Vec<AdapterFix>,
    /// `HiberbootEnabled = 0`, a separate choice offered while Fast Startup is active.
    pub fast_startup_off: bool,
    /// Adapters that need a change the fix can't make, and why.
    #[serde(default)]
    pub skipped: Vec<String>,
}

impl FixPlan {
    /// The changes for the wired physical adapters, or the Wi-Fi ones on a PC
    /// without any. `fast_startup_off` is kept only while Fast Startup is
    /// active.
    pub fn new(
        adapters: &[Adapter],
        report: &NicReport,
        system: &SystemFacts,
        fast_startup_off: bool,
    ) -> Self {
        let physical: Vec<_> = adapters.iter().filter(|a| a.physical()).collect();
        let wired = physical.iter().any(|a| a.medium == Medium::Wired);
        let mut plan = Self {
            fast_startup_off: fast_startup_off && system.fast_startup_active(),
            ..Self::default()
        };
        for adapter in physical {
            if wired && adapter.medium != Medium::Wired {
                continue;
            }
            let Some(nic) = report.get(adapter) else {
                continue;
            };
            let mut fix = AdapterFix {
                guid: nic.guid.clone(),
                pnp: nic.pnp.clone(),
                name: nic.name.clone(),
                wake_enable: nic.wake_enable == Some(false),
                magic_packet_only: nic.magic_packet_only == Some(false),
                ..Default::default()
            };
            let mut set = |keyword: &str| {
                if nic.keyword(keyword).is_some_and(|value| value != "1") {
                    fix.keywords.insert(keyword.into(), "1".into());
                }
            };
            set(MAGIC_PACKET);
            if system.modern_standby {
                set(MODERN_STANDBY_MAGIC_PACKET);
            }
            if !(fix.wake_enable || fix.magic_packet_only || !fix.keywords.is_empty()) {
                continue;
            }
            // The fix finds the adapter's settings by its PnP id, so it can't
            // change them without one.
            if fix.pnp.trim().is_empty() {
                plan.skipped.push(format!(
                    "{}: Windows reports no device id for it, so its wake settings can't be changed",
                    fix.name
                ));
                continue;
            }
            plan.adapters.push(fix);
        }
        plan
    }
    pub fn is_empty(&self) -> bool {
        self.adapters.is_empty() && !self.fast_startup_off
    }
    /// Why a plan handed to the child can't be one [`FixPlan::new`] made: it
    /// may only turn wake on, never set another advanced property.
    pub fn refusal(&self) -> Option<String> {
        self.adapters.iter().find_map(|fix| {
            fix.keywords
                .iter()
                .find(|(keyword, value)| {
                    ![MAGIC_PACKET, MODERN_STANDBY_MAGIC_PACKET].contains(&keyword.as_str())
                        || value.as_str() != "1"
                })
                .map(|(keyword, value)| {
                    format!("{}: refusing to set {keyword} to {value:?}", fix.name)
                })
        })
    }
    /// Each change in plain words, for the installer, the console and the log.
    pub fn changes(&self) -> Vec<String> {
        let mut changes = Vec::new();
        for fix in &self.adapters {
            if fix.wake_enable {
                changes.push(format!(
                    "{}: allow this device to wake the computer",
                    fix.name
                ));
            }
            if fix.keywords.contains_key(MAGIC_PACKET) {
                changes.push(format!("{}: wake on magic packet", fix.name));
            }
            if fix.keywords.contains_key(MODERN_STANDBY_MAGIC_PACKET) {
                changes.push(format!("{}: wake from Modern Standby", fix.name));
            }
            if fix.magic_packet_only {
                changes.push(format!(
                    "{}: only allow a magic packet to wake the computer",
                    fix.name
                ));
            }
            if !fix.keywords.is_empty() {
                changes.push(format!(
                    "{}: restarts, so the network drops for a few seconds",
                    fix.name
                ));
            }
        }
        if self.fast_startup_off {
            changes.push("Turn off Fast Startup".into());
        }
        changes
    }
}

/// What carrying out a [`FixPlan`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ApplyReport {
    /// The adapters restarted for a changed advanced property.
    pub restarted: Vec<String>,
    /// The adapters whose changed property takes effect at the next restart
    /// of the PC: the fix was too close to its deadline to restart them.
    pub after_reboot: Vec<String>,
    pub errors: Vec<String>,
}

#[cfg(test)]
mod tests;
