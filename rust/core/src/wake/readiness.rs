//! Wake-on-LAN readiness: judging what `wake_nics.ps1 read` reports, and the
//! plan its `apply` mode carries out (`docs/wake-and-audio-design.md`, A.3).
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

/// What `wake_nics.ps1 read` prints.
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
/// reason when the script failed or didn't report the adapter.
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
/// with the reason when the NIC script failed.
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
                    .ok_or("the NIC script did not report this adapter"),
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

/// What `wake_nics.ps1 apply` changes, with consent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixPlan {
    pub adapters: Vec<AdapterFix>,
    /// `HiberbootEnabled = 0`, a separate choice offered while Fast Startup is active.
    pub fast_startup_off: bool,
    /// Adapters that need a change the script can't make, and why.
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
            // The script finds the WMI settings by PnP id; an empty one would
            // match every adapter.
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

/// What `wake_nics.ps1 apply` prints.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ApplyReport {
    /// The adapters restarted for a changed advanced property.
    pub restarted: Vec<String>,
    /// The adapters whose changed property takes effect at the next restart
    /// of the PC: the script was too close to its deadline to restart them.
    pub after_reboot: Vec<String>,
    pub errors: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn this_pc() -> Vec<Adapter> {
        serde_json::from_str(include_str!("fixtures/adapters_this_pc.json")).unwrap()
    }
    fn report(name: &str) -> NicReport {
        NicReport::parse(match name {
            "de" => include_str!("fixtures/nics_read_de.json"),
            "en" => include_str!("fixtures/nics_read_en.json"),
            _ => unreachable!("{name}"),
        })
        .unwrap()
    }
    fn state(checks: &[Check], id: &str) -> Option<CheckState> {
        checks.iter().find(|c| c.id == id).map(|c| c.state)
    }
    const S3: SystemFacts = SystemFacts {
        s3: true,
        modern_standby: false,
        hibernate: false,
        hiberboot: Some(true),
    };

    #[test]
    fn this_pcs_realtek_is_ready_in_any_language() {
        let adapters = this_pc();
        for language in ["de", "en"] {
            let report = report(language);
            let local = Some("192.168.4.20".parse().unwrap());
            let (primary, targets) = targets(&adapters, Ok(&report), false, local);
            assert_eq!(primary.as_deref(), Some("02:7C:16:B4:13:4F"), "{language}");
            // The Realtek and the disconnected WLAN; never vEthernet or Wi-Fi Direct.
            assert_eq!(targets.len(), 2, "{language}");
            let realtek = &targets[0];
            assert_eq!(realtek.kind, AdapterKind::Ethernet);
            assert!(realtek.route && realtek.connected);
            assert_eq!(realtek.broadcasts, ["192.168.4.255"]);
            assert_eq!(realtek.state, TargetState::Ready, "{language}");
            assert_eq!(
                state(&realtek.checks, check::WAKE_ENABLED),
                Some(CheckState::Ok)
            );
            assert_eq!(
                state(&realtek.checks, check::MAGIC_PACKET),
                Some(CheckState::Ok)
            );
            assert_eq!(
                state(&realtek.checks, check::MAGIC_PACKET_ONLY),
                Some(CheckState::Warning)
            );
            assert_eq!(
                state(&realtek.checks, check::WAKE_ON_PATTERN),
                Some(CheckState::Ok)
            );
            assert_eq!(
                state(&realtek.checks, check::SHUTDOWN_WAKE),
                Some(CheckState::Ok)
            );
            // Not on a Modern Standby PC.
            assert_eq!(
                state(&realtek.checks, check::MODERN_STANDBY_MAGIC_PACKET),
                None
            );
            let wlan = &targets[1];
            assert_eq!(wlan.kind, AdapterKind::Wifi);
            assert_eq!(wlan.state, TargetState::Experimental);
            assert!(!wlan.route && !wlan.connected);
        }
    }

    #[test]
    fn the_english_and_german_reports_differ_only_in_localized_text() {
        let (de, en) = (report("de"), report("en"));
        assert_eq!(de.adapters.len(), en.adapters.len());
        let localized = |r: &NicReport| -> Vec<_> {
            r.adapters
                .iter()
                .map(|a| (a.name.clone(), a.errors.clone()))
                .collect()
        };
        assert_ne!(localized(&de), localized(&en));
        for (de, en) in de.adapters.iter().zip(&en.adapters) {
            assert_eq!(
                (de.guid.as_str(), &de.keywords, de.wake_enable),
                (en.guid.as_str(), &en.keywords, en.wake_enable)
            );
            assert_eq!(de.magic_packet_only, en.magic_packet_only);
        }
    }

    #[test]
    fn settings_that_are_off_need_setup_and_unreadable_ones_are_unknown() {
        let adapters = this_pc();
        let realtek = &adapters[0];
        let mut nic = report("en").get(realtek).unwrap().clone();
        nic.wake_enable = Some(false);
        let (state, checks) = judge(realtek, Ok(&nic), false);
        assert_eq!(state, TargetState::NeedsSetup);
        assert_eq!(state_of(&checks, check::WAKE_ENABLED), CheckState::Error);
        nic.wake_enable = None;
        assert_eq!(judge(realtek, Ok(&nic), false).0, TargetState::Unknown);
        nic.wake_enable = Some(true);
        nic.keywords
            .insert(MAGIC_PACKET.to_ascii_uppercase(), "0".into());
        assert_eq!(judge(realtek, Ok(&nic), false).0, TargetState::NeedsSetup);
        // Modern Standby needs its own keyword when the adapter has it.
        nic.keywords
            .insert(MAGIC_PACKET.to_ascii_uppercase(), "1".into());
        nic.keywords
            .insert(MODERN_STANDBY_MAGIC_PACKET.to_ascii_uppercase(), "0".into());
        assert_eq!(judge(realtek, Ok(&nic), false).0, TargetState::Ready);
        assert_eq!(judge(realtek, Ok(&nic), true).0, TargetState::NeedsSetup);
        // A blocked script: every check unknown, never ready.
        let (state, checks) = judge(realtek, Err("PowerShell is blocked"), false);
        assert_eq!(state, TargetState::Unknown);
        assert!(checks.iter().all(|c| c.state == CheckState::Unknown));
        let (primary, targets) = targets(&adapters, Err("blocked"), false, None);
        assert_eq!(primary, None);
        assert!(targets.iter().all(|t| t.state != TargetState::Ready));
    }
    fn state_of(checks: &[Check], id: &str) -> CheckState {
        state(checks, id).unwrap()
    }

    #[test]
    fn keywords_are_matched_in_any_case_and_a_bom_is_ignored() {
        let report = NicReport::parse(
            "\u{feff}{\"adapters\":[{\"guid\":\"{875511c7-0ef5-45f7-876c-3be50726ac42}\",\
             \"keywords\":{\"*wakeonmagicpacket\":\" 1 \"},\"wake_enable\":true}]}",
        )
        .unwrap();
        let nic = report.get(&this_pc()[0]).unwrap();
        assert_eq!(nic.keyword(MAGIC_PACKET), Some("1"));
        assert!(nic.armed());
        // An empty keyword map comes out of PowerShell as `{}` or `null`.
        let report = NicReport::parse("{\"adapters\":[{\"keywords\":null}]}").unwrap();
        assert!(report.adapters[0].keywords.is_empty());
    }

    #[test]
    fn system_facts_map_to_the_api() {
        let system = system(Some(&S3));
        assert_eq!(system.sleep, Sleep::S3);
        // Hiberboot without a hibernation file has no effect.
        assert_eq!(system.fast_startup, FastStartup::Inactive);
        assert_eq!(system.shutdown_wake, ShutdownWake::FirmwareUnknown);
        let hibernate = SystemFacts {
            hibernate: true,
            ..S3
        };
        assert_eq!(
            super::system(Some(&hibernate)).fast_startup,
            FastStartup::Active
        );
        let off = SystemFacts {
            hiberboot: Some(false),
            ..hibernate
        };
        assert_eq!(super::system(Some(&off)).fast_startup, FastStartup::Off);
        let standby = SystemFacts {
            s3: false,
            modern_standby: true,
            ..S3
        };
        assert_eq!(super::system(Some(&standby)).sleep, Sleep::None);
        assert_eq!(super::system(None).sleep, Sleep::Unknown);
    }

    #[test]
    fn the_fix_plan_names_each_keyword_and_wmi_change() {
        let adapters = this_pc();
        // This PC: nothing but "magic packet only", which is off.
        let plan = FixPlan::new(&adapters, &report("de"), &S3, true);
        assert_eq!(
            plan,
            FixPlan {
                adapters: vec![AdapterFix {
                    guid: "{875511C7-0EF5-45F7-876C-3BE50726AC42}".into(),
                    pnp: "PCI\\VEN_10EC&DEV_8125&SUBSYS_7C911462&REV_05\\0100000000000000000"
                        .into(),
                    name: "Ethernet".into(),
                    keywords: BTreeMap::new(),
                    wake_enable: false,
                    magic_packet_only: true,
                }],
                // Fast Startup isn't active without hibernate.
                fast_startup_off: false,
                skipped: Vec::new(),
            }
        );
        assert_eq!(
            plan.changes(),
            ["Ethernet: only allow a magic packet to wake the computer"]
        );
        // Everything off on a Modern Standby PC with Fast Startup active.
        let mut report = report("en");
        let realtek = report
            .adapters
            .iter_mut()
            .find(|a| a.name == "Ethernet")
            .unwrap();
        realtek.wake_enable = Some(false);
        realtek
            .keywords
            .insert(MAGIC_PACKET.to_ascii_uppercase(), "0".into());
        realtek
            .keywords
            .insert(MODERN_STANDBY_MAGIC_PACKET.to_ascii_uppercase(), "0".into());
        let standby = SystemFacts {
            s3: false,
            modern_standby: true,
            hibernate: true,
            hiberboot: Some(true),
        };
        let plan = FixPlan::new(&adapters, &report, &standby, true);
        assert_eq!(
            plan.adapters.len(),
            1,
            "the WLAN isn't touched on a PC with Ethernet"
        );
        let fix = &plan.adapters[0];
        assert!(fix.wake_enable && fix.magic_packet_only);
        assert_eq!(
            fix.keywords,
            BTreeMap::from([
                (MAGIC_PACKET.into(), "1".into()),
                (MODERN_STANDBY_MAGIC_PACKET.into(), "1".into()),
            ])
        );
        assert!(plan.fast_startup_off);
        assert_eq!(
            plan.changes(),
            [
                "Ethernet: allow this device to wake the computer",
                "Ethernet: wake on magic packet",
                "Ethernet: wake from Modern Standby",
                "Ethernet: only allow a magic packet to wake the computer",
                "Ethernet: restarts, so the network drops for a few seconds",
                "Turn off Fast Startup",
            ]
        );
        // The plan travels to the script as JSON with the registry's keywords.
        let json = serde_json::to_string(&plan).unwrap();
        assert!(json.contains("\"*WakeOnMagicPacket\":\"1\""), "{json}");
        // Without consent to Fast Startup, or when all is set, nothing changes.
        assert!(!FixPlan::new(&adapters, &report, &standby, false).fast_startup_off);
        let mut set = super::NicReport::parse(include_str!("fixtures/nics_read_de.json")).unwrap();
        set.adapters
            .iter_mut()
            .for_each(|a| a.magic_packet_only = Some(true));
        assert!(FixPlan::new(&adapters, &set, &S3, true).is_empty());
    }

    #[test]
    fn a_wifi_only_pc_fixes_its_wifi() {
        let laptop: Vec<Adapter> =
            serde_json::from_str(include_str!("fixtures/adapters_wifi_laptop.json")).unwrap();
        let report = NicReport::parse(
            r#"{"adapters":[{"guid":"{00000000-0000-4000-8000-000000000112}","pnp":"PCI\\VEN_14C3","name":"Wi-Fi","wake_enable":false,"magic_packet_only":null,"keywords":{"*WakeOnMagicPacket":"1"}}]}"#,
        )
        .unwrap();
        let plan = FixPlan::new(&laptop, &report, &S3, false);
        assert_eq!(plan.adapters.len(), 1);
        assert!(plan.adapters[0].wake_enable && plan.adapters[0].keywords.is_empty());
    }

    #[test]
    fn a_plan_may_only_turn_wake_on() {
        let modern = SystemFacts {
            modern_standby: true,
            ..S3
        };
        let mut report = report("en");
        report.adapters.iter_mut().for_each(|nic| {
            nic.keywords
                .values_mut()
                .for_each(|value| *value = "0".into())
        });
        let plan = FixPlan::new(&this_pc(), &report, &modern, false);
        assert!(!plan.adapters[0].keywords.is_empty(), "{plan:?}");
        assert_eq!(plan.refusal(), None);
        let mut forged = plan.clone();
        forged.adapters[0]
            .keywords
            .insert("*SpeedDuplex".into(), "1".into());
        assert!(forged.refusal().unwrap().contains("*SpeedDuplex"));
        let mut forged = plan;
        forged.adapters[0]
            .keywords
            .insert(MAGIC_PACKET.into(), "0".into());
        assert!(forged.refusal().unwrap().contains("\"0\""));
    }

    #[test]
    fn an_adapter_without_a_pnp_id_is_skipped_with_the_reason() {
        let adapters = this_pc();
        let mut report = report("en");
        let realtek = report
            .adapters
            .iter_mut()
            .find(|a| a.name == "Ethernet")
            .unwrap();
        // Its WMI instances can't be told apart from every other adapter's.
        realtek.pnp = " ".into();
        realtek.wake_enable = Some(false);
        let plan = FixPlan::new(&adapters, &report, &S3, false);
        assert!(plan.adapters.is_empty() && plan.is_empty());
        assert_eq!(
            plan.skipped,
            [
                "Ethernet: Windows reports no device id for it, so its wake settings can't be changed"
            ]
        );
        assert!(plan.changes().is_empty());
        // Nothing to change on it: nothing to report either.
        let mut set = report.clone();
        set.adapters.iter_mut().for_each(|a| {
            a.wake_enable = Some(true);
            a.magic_packet_only = Some(true);
        });
        assert!(FixPlan::new(&adapters, &set, &S3, false).skipped.is_empty());
    }
}
