//! NVIDIA driver settings for the GPU settings panel: the NVIDIA counterpart of the Radeon
//! settings (`radeon`), served through the same worker, journal and restore.
//!
//! The settings are the NVIDIA Control Panel's global 3D settings, read and written in the DRS
//! global profile through NVAPI (`nvidia_profile::NvApi`). Every call opens a fresh DRS session,
//! so a change made in the NVIDIA Control Panel or NVIDIA App between two calls is seen. A
//! stream's changes are journaled first and put back when it ends, as the Radeon ones are.
//!
//! Setting ids and values come from NVIDIA's MIT-licensed `NvApiDriverSettings.h`, except the
//! Low Latency Mode and Smooth Motion keys, which the header does not list: those are the ones
//! the NVIDIA Control Panel and NVIDIA App write, under the names NVIDIA Profile Inspector gives
//! them ("Ultra Low Latency - CPL State", "Ultra Low Latency - Enabled", "Smooth Motion -
//! Enable", "Smooth Motion - Allowed APIs"), as Vibepollo (Nonary, GPL-3.0) wrote them.
//!
//! A combination the panel does not offer (a pre-rendered frame limit of 3 from NVIDIA Profile
//! Inspector, a half-rate V-Sync) reads as read-only `custom`, so the host never overwrites what
//! it could not put back.

use crate::radeon::{
    BackendError, GpuInfo, RadeonBackend,
    model::{ChoiceOption, Kind, Range, Setting, State, Value, Writable},
};
use anyhow::Result;
use std::collections::BTreeMap;

/// The journal of NVIDIA changes, beside the Radeon one, so a PC that changes GPU vendor never
/// replays one vendor's entries on the other.
pub const JOURNAL_FILE: &str = "nvidia-journal.json";

const PRERENDERLIMIT: u32 = 0x007B_A09E;
const ULL_CPL_STATE: u32 = 0x0005_F543;
const ULL_ENABLED: u32 = 0x1083_5000;
const FRL_FPS: u32 = 0x1083_5002;
const FRL_FPS_MAX: i64 = 0x3FF;
const VSYNCMODE: u32 = 0x00A8_79CF;
const PREFERRED_PSTATE: u32 = 0x1057_EB71;
const QUALITY_ENHANCEMENTS: u32 = 0x00CE_2691;
const ANISO_MODE_SELECTOR: u32 = 0x10D2_BB16;
const ANISO_MODE_LEVEL: u32 = 0x101E_61A9;
const SMOOTH_MOTION: u32 = 0xB0D3_84C0;
const SMOOTH_MOTION_APIS: u32 = 0xB0CC_0875;
/// DirectX 11, DirectX 12 and Vulkan, what the NVIDIA App allows when it turns Smooth Motion on.
const SMOOTH_MOTION_ALL_APIS: u32 = 7;
/// Smooth Motion came with driver 571.86 (RTX 50; RTX 40 from 576.02).
const SMOOTH_MOTION_DRIVER: u32 = 57186;

/// What each key reads when the global profile does not store it: the header's defaults.
fn default_of(id: u32) -> u32 {
    match id {
        VSYNCMODE => VSYNC[0].0,
        PREFERRED_PSTATE => 5,
        ANISO_MODE_LEVEL => 1,
        _ => 0,
    }
}

const VSYNC: &[(u32, &str)] = &[
    (0x6092_5292, "application"),
    (0x0841_6747, "off"),
    (0x4781_4940, "on"),
    (0x1888_8888, "fast"),
];
const POWER: &[(u32, &str)] = &[
    (5, "optimal_power"),
    (0, "adaptive"),
    (1, "prefer_maximum_performance"),
    (3, "prefer_consistent_performance"),
];
const TEXTURE_QUALITY: &[(u32, &str)] = &[
    (0xFFFF_FFF6, "high_quality"),
    (0, "quality"),
    (10, "performance"),
    (20, "high_performance"),
];
const ANISO: &[(u32, &str)] = &[(2, "2x"), (4, "4x"), (8, "8x"), (16, "16x")];
/// Low Latency Mode as the NVIDIA Control Panel writes it: the pre-rendered frame limit, the
/// panel's own state and Ultra's switch.
const LOW_LATENCY: &[((u32, u32, u32), &str)] =
    &[((0, 0, 0), "off"), ((1, 1, 0), "on"), ((1, 2, 1), "ultra")];

/// The DWORD settings of the DRS global profile.
pub trait Store {
    /// The stored value, None when the global profile does not store the key.
    fn get(&mut self, id: u32) -> Result<Option<u32>>;
    fn set(&mut self, id: u32, value: u32) -> Result<()>;
    fn save(&mut self) -> Result<()>;
    /// The driver version, 57186 for 571.86.
    fn driver(&mut self) -> Option<u32>;
}

fn value(store: &mut impl Store, id: u32) -> Result<u32> {
    Ok(store.get(id)?.unwrap_or_else(|| default_of(id)))
}

fn options(names: impl IntoIterator<Item = &'static str>) -> Vec<ChoiceOption> {
    names
        .into_iter()
        .enumerate()
        .map(|(raw, name)| ChoiceOption {
            value: name.into(),
            raw: raw as i32,
        })
        .collect()
}

fn setting(id: &'static str, label: &'static str, kind: Kind) -> Setting {
    Setting {
        id,
        group: id.split('.').next().unwrap_or(id),
        label,
        interface: "NVDRS global profile",
        kind,
        state: State::Configurable,
        supported: Some(true),
        readable: Some(true),
        writable: Writable::Yes,
        value: None,
        unit: None,
        range: None,
        options: Vec::new(),
        depends: Vec::new(),
        notes: Vec::new(),
        adlx: BTreeMap::new(),
    }
}

/// A choice read as `known`, or, when the driver holds a combination the panel doesn't offer,
/// read-only `custom` with why.
fn choice(
    mut setting: Setting,
    names: Vec<&'static str>,
    known: Option<&'static str>,
    raw: String,
) -> Setting {
    setting.options = options(names);
    match known {
        Some(name) => setting.value = Some(Value::Choice(name.into())),
        None => {
            setting.value = Some(Value::Choice("custom".into()));
            setting.state = State::ReadOnly;
            setting.writable = Writable::No;
            setting.notes.push(format!(
                "set to {raw} outside these options (NVIDIA Profile Inspector or an older driver); Rubylight leaves it as it is"
            ));
        }
    }
    setting
}

/// The settings as the driver stores them now.
pub fn read(store: &mut impl Store) -> Result<Vec<Setting>> {
    let mut settings = Vec::new();

    let latency = (
        value(store, PRERENDERLIMIT)?,
        value(store, ULL_CPL_STATE)?,
        value(store, ULL_ENABLED)?,
    );
    let known = LOW_LATENCY
        .iter()
        .find(|(raw, _)| *raw == latency)
        .map(|(_, name)| *name);
    settings.push(choice(
        setting("nvidia_low_latency.mode", "Low Latency Mode", Kind::Choice),
        LOW_LATENCY.iter().map(|(_, name)| *name).collect(),
        known,
        format!(
            "pre-rendered frames {}, state {}, ultra {}",
            latency.0, latency.1, latency.2
        ),
    ));

    let mut limit = setting(
        "nvidia_frame_rate_limit.fps",
        "Max Frame Rate",
        Kind::Integer,
    );
    limit.value = Some(Value::Int(i64::from(value(store, FRL_FPS)?)));
    limit.range = Some(Range {
        min: 0,
        max: FRL_FPS_MAX,
        step: 1,
    });
    limit.unit = Some("fps");
    limit.notes.push("0 is off".into());
    settings.push(limit);

    let mut table_choice = |id, label, table: &[(u32, &'static str)], key| -> Result<Setting> {
        let raw = value(store, key)?;
        let known = table
            .iter()
            .find(|(value, _)| *value == raw)
            .map(|(_, name)| *name);
        Ok(choice(
            setting(id, label, Kind::Choice),
            table.iter().map(|(_, name)| *name).collect(),
            known,
            format!("{raw:#x}"),
        ))
    };
    settings.push(table_choice(
        "nvidia_vsync.mode",
        "Vertical Sync",
        VSYNC,
        VSYNCMODE,
    )?);
    settings.push(table_choice(
        "nvidia_power.mode",
        "Power Management Mode",
        POWER,
        PREFERRED_PSTATE,
    )?);
    settings.push(table_choice(
        "nvidia_texture_filtering.quality",
        "Texture Filtering Quality",
        TEXTURE_QUALITY,
        QUALITY_ENHANCEMENTS,
    )?);

    let selector = value(store, ANISO_MODE_SELECTOR)?;
    let level = value(store, ANISO_MODE_LEVEL)?;
    let known = match selector {
        0 => Some("application"),
        1 => ANISO
            .iter()
            .find(|(raw, _)| *raw == level)
            .map(|(_, name)| *name),
        _ => None,
    };
    settings.push(choice(
        setting(
            "nvidia_anisotropic_filtering.mode",
            "Anisotropic Filtering",
            Kind::Choice,
        ),
        std::iter::once("application")
            .chain(ANISO.iter().map(|(_, name)| *name))
            .collect(),
        known,
        format!("mode {selector}, level {level}"),
    ));

    let mut smooth = setting(
        "nvidia_smooth_motion.enabled",
        "Smooth Motion",
        Kind::Toggle,
    );
    smooth.value = Some(Value::Bool(value(store, SMOOTH_MOTION)? == 1));
    smooth.notes.push("RTX 40 and 50 series".into());
    match store.driver() {
        Some(driver) if driver < SMOOTH_MOTION_DRIVER => {
            smooth.state = State::NeedsExternalSetup;
            smooth.writable = Writable::No;
            smooth.notes.push(format!(
                "needs NVIDIA driver 571.86 or newer; this one is {}.{:02}",
                driver / 100,
                driver % 100
            ));
        }
        _ => {}
    }
    settings.push(smooth);
    Ok(settings)
}

fn named(table: &[(u32, &'static str)], name: &str, id: &str) -> Result<u32, BackendError> {
    table
        .iter()
        .find(|(_, item)| *item == name)
        .map(|(raw, _)| *raw)
        .ok_or_else(|| BackendError(format!("{name} is not a value of {id}")))
}

/// Writes one validated value and saves the profile.
pub fn write(store: &mut impl Store, id: &str, value: &Value) -> Result<(), BackendError> {
    let fail = |e: anyhow::Error| BackendError(format!("{e:#}"));
    let writes: Vec<(u32, u32)> = match (id, value) {
        ("nvidia_low_latency.mode", Value::Choice(name)) => {
            let (prerender, state, ultra) = LOW_LATENCY
                .iter()
                .find(|(_, item)| item == name)
                .map(|(raw, _)| *raw)
                .ok_or_else(|| BackendError(format!("{name} is not a Low Latency Mode")))?;
            vec![
                (PRERENDERLIMIT, prerender),
                (ULL_CPL_STATE, state),
                (ULL_ENABLED, ultra),
            ]
        }
        ("nvidia_frame_rate_limit.fps", Value::Int(fps)) if (0..=FRL_FPS_MAX).contains(fps) => {
            vec![(FRL_FPS, *fps as u32)]
        }
        ("nvidia_vsync.mode", Value::Choice(name)) => vec![(VSYNCMODE, named(VSYNC, name, id)?)],
        ("nvidia_power.mode", Value::Choice(name)) => {
            vec![(PREFERRED_PSTATE, named(POWER, name, id)?)]
        }
        ("nvidia_texture_filtering.quality", Value::Choice(name)) => {
            vec![(QUALITY_ENHANCEMENTS, named(TEXTURE_QUALITY, name, id)?)]
        }
        ("nvidia_anisotropic_filtering.mode", Value::Choice(name)) if name == "application" => {
            vec![(ANISO_MODE_SELECTOR, 0)]
        }
        ("nvidia_anisotropic_filtering.mode", Value::Choice(name)) => {
            vec![
                (ANISO_MODE_SELECTOR, 1),
                (ANISO_MODE_LEVEL, named(ANISO, name, id)?),
            ]
        }
        ("nvidia_smooth_motion.enabled", Value::Bool(on)) => {
            let mut writes = vec![(SMOOTH_MOTION, u32::from(*on))];
            // Allow every API only when nothing chose which: a mask set elsewhere stays.
            if *on && store.get(SMOOTH_MOTION_APIS).map_err(fail)?.unwrap_or(0) == 0 {
                writes.push((SMOOTH_MOTION_APIS, SMOOTH_MOTION_ALL_APIS));
            }
            writes
        }
        _ => {
            return Err(BackendError(format!(
                "{id} cannot take {}",
                value.display()
            )));
        }
    };
    for (key, raw) in writes {
        store.set(key, raw).map_err(fail)?;
    }
    store.save().map_err(fail)
}

/// `NvApi`'s global profile as a [`Store`].
struct Global {
    api: crate::nvidia_profile::NvApi,
    profile: crate::nvidia_profile::Handle,
}

impl Global {
    fn open() -> Result<Self, BackendError> {
        let api = crate::nvidia_profile::NvApi::open()
            .map_err(|e| {
                BackendError(format!(
                    "the NVIDIA driver settings could not be opened: {e:#}"
                ))
            })?
            .ok_or_else(|| BackendError("this PC has no NVIDIA driver".into()))?;
        let profile = api
            .global_profile()
            .map_err(|e| BackendError(format!("{e:#}")))?;
        Ok(Self { api, profile })
    }
}

impl Store for Global {
    fn get(&mut self, id: u32) -> Result<Option<u32>> {
        use crate::nvidia_profile::Drs;
        Ok(self.api.setting(self.profile, id)?.map(|v| v.value))
    }
    fn set(&mut self, id: u32, value: u32) -> Result<()> {
        use crate::nvidia_profile::Drs;
        self.api.set_setting(self.profile, id, value)
    }
    fn save(&mut self) -> Result<()> {
        use crate::nvidia_profile::Drs;
        self.api.save()
    }
    fn driver(&mut self) -> Option<u32> {
        self.api.driver_version()
    }
}

/// The NVIDIA GPUs of this PC, numbered in DXGI's order, with the driver version.
fn nvidia_gpus() -> Result<Vec<GpuInfo>, BackendError> {
    let gpus = crate::capture::gpus().map_err(|e| BackendError(format!("{e:#}")))?;
    let driver = Global::open().ok().and_then(|mut global| global.driver());
    Ok(gpus
        .into_iter()
        .filter(|gpu| butterpollo_core::encoder_gpu::is_nvidia(gpu.vendor))
        .enumerate()
        .map(|(index, gpu)| GpuInfo {
            id: index as i32,
            name: gpu.name,
            driver: driver.map_or_else(String::new, |d| format!("{}.{:02}", d / 100, d % 100)),
        })
        .collect())
}

/// The real backend. The driver settings are the same for every NVIDIA GPU of the PC, so `gpu`
/// only names which one the panel shows.
pub struct NvidiaBackend;

impl RadeonBackend for NvidiaBackend {
    fn read(&mut self, _gpu: i32) -> Result<Vec<Setting>, BackendError> {
        read(&mut Global::open()?).map_err(|e| BackendError(format!("{e:#}")))
    }
    fn write(&mut self, _gpu: i32, id: &str, value: &Value) -> Result<(), BackendError> {
        write(&mut Global::open()?, id, value)
    }
    fn gpus(&mut self) -> Result<Vec<GpuInfo>, BackendError> {
        nvidia_gpus()
    }
}

#[cfg(test)]
mod tests;
