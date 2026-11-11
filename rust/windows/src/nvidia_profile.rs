//! The NVIDIA driver profile that keeps the GPU's clocks up while the host
//! encodes.
//!
//! NVIDIA's adaptive power management lowers the clocks of a GPU whose 3D
//! load is light, and NVENC slows down with them: a desktop or a light game
//! then encodes far slower than the same GPU under load. Sunshine and the
//! hosts derived from it put their executable in a driver profile that
//! prefers maximum performance (`nvenc_latency_over_power`, on by default),
//! and so does this one. The profile names only `butterpollo.exe`; an idle
//! host holds no GPU context, so the clocks are raised while it streams or
//! probes, not at idle and not for games.
//!
//! The order of the DRS calls (profile, application, setting, save only when
//! something changed, the setting removed when the option is off) follows
//! Sunshine's nvprefs (LizardByte/Sunshine, GPL-3.0,
//! src/platform/windows/nvprefs/driver_settings.cpp). Interface IDs, layouts
//! and values come from NVIDIA's MIT-licensed NVAPI headers (nvapi.h,
//! nvapi_interface.h, NvApiDriverSettings.h).

use anyhow::{Result, anyhow, bail};
use std::ffi::c_void;

/// The driver profile this host creates.
pub const PROFILE_NAME: &str = "Rubylight";
/// The executable the profile applies to.
pub const APPLICATION: &str = "butterpollo.exe";
/// `PREFERRED_PSTATE_ID`, "Power management mode".
pub const PREFERRED_PSTATE: u32 = 0x1057_EB71;
/// `PREFERRED_PSTATE_PREFER_MAX`, "Prefer maximum performance".
pub const PREFER_MAX: u32 = 1;

/// A setting's value and whether it is stored in the profile itself rather
/// than inherited from the global or base profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Value {
    pub value: u32,
    pub own: bool,
}

/// The driver settings store, reduced to what the profile needs.
pub trait Drs {
    type Profile: Copy;
    fn find_profile(&mut self, name: &str) -> Result<Option<Self::Profile>>;
    fn create_profile(&mut self, name: &str) -> Result<Self::Profile>;
    fn has_application(&mut self, profile: Self::Profile, name: &str) -> Result<bool>;
    fn create_application(&mut self, profile: Self::Profile, name: &str) -> Result<()>;
    fn setting(&mut self, profile: Self::Profile, id: u32) -> Result<Option<Value>>;
    fn set_setting(&mut self, profile: Self::Profile, id: u32, value: u32) -> Result<()>;
    fn delete_setting(&mut self, profile: Self::Profile, id: u32) -> Result<()>;
    fn save(&mut self) -> Result<()>;
}

/// What [`apply`] changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Unchanged,
    /// The profile now prefers maximum performance for the host.
    Set,
    /// The option is off and the profile's power setting was removed.
    Removed,
}

/// Brings the profile in line with `latency_over_power`, saving only when
/// something changed.
pub fn apply<D: Drs>(drs: &mut D, latency_over_power: bool) -> Result<Change> {
    if !latency_over_power {
        let Some(profile) = drs.find_profile(PROFILE_NAME)? else {
            return Ok(Change::Unchanged);
        };
        return match drs.setting(profile, PREFERRED_PSTATE)? {
            Some(Value { own: true, .. }) => {
                drs.delete_setting(profile, PREFERRED_PSTATE)?;
                drs.save()?;
                Ok(Change::Removed)
            }
            _ => Ok(Change::Unchanged),
        };
    }
    let mut modified = false;
    let profile = match drs.find_profile(PROFILE_NAME)? {
        Some(profile) => profile,
        None => {
            modified = true;
            drs.create_profile(PROFILE_NAME)?
        }
    };
    if !drs.has_application(profile, APPLICATION)? {
        drs.create_application(profile, APPLICATION)?;
        modified = true;
    }
    let wanted = Value {
        value: PREFER_MAX,
        own: true,
    };
    if drs.setting(profile, PREFERRED_PSTATE)? != Some(wanted) {
        drs.set_setting(profile, PREFERRED_PSTATE, PREFER_MAX)?;
        modified = true;
    }
    if !modified {
        return Ok(Change::Unchanged);
    }
    drs.save()?;
    Ok(Change::Set)
}

/// Applies `nvenc_latency_over_power` (on by default) to the driver profile.
/// Runs at startup, before this process opens a device on the NVIDIA GPU,
/// since the driver reads a process's profile when it first loads there. A
/// PC without an NVIDIA driver is skipped; a failure is logged as a warning.
pub fn apply_configured(config: &butterpollo_core::config::Config) {
    let wanted = config.boolean("nvenc_latency_over_power", true);
    let mut api = match NvApi::open() {
        Ok(Some(api)) => api,
        Ok(None) => {
            tracing::debug!("no NVIDIA driver; the NVIDIA power profile is not needed");
            return;
        }
        Err(error) => {
            tracing::warn!(
                error = format!("{error:#}"),
                "the NVIDIA driver settings could not be opened; NVENC may run at reduced clocks when the GPU is lightly loaded"
            );
            return;
        }
    };
    match apply(&mut api, wanted) {
        Ok(Change::Set) => tracing::info!(
            profile = PROFILE_NAME,
            application = APPLICATION,
            "NVIDIA power management for the host set to prefer maximum performance (nvenc_latency_over_power)"
        ),
        Ok(Change::Removed) => tracing::info!(
            profile = PROFILE_NAME,
            "nvenc_latency_over_power is off; the host's NVIDIA power management setting was removed"
        ),
        Ok(Change::Unchanged) => tracing::debug!(
            latency_over_power = wanted,
            "NVIDIA power profile already up to date"
        ),
        Err(error) => tracing::warn!(
            error = format!("{error:#}"),
            "the NVIDIA power profile could not be written; NVENC may run at reduced clocks when the GPU is lightly loaded. Set nvenc_latency_over_power = false to stop trying"
        ),
    }
}

const UNICODE_MAX: usize = 2048;
const OK: i32 = 0;
const LIBRARY_NOT_FOUND: i32 = -2;
const NVIDIA_DEVICE_NOT_FOUND: i32 = -6;
const SETTING_NOT_FOUND: i32 = -160;
const PROFILE_NOT_FOUND: i32 = -163;
const EXECUTABLE_NOT_FOUND: i32 = -166;
const DWORD_TYPE: u32 = 0;
const CURRENT_PROFILE_LOCATION: u32 = 0;

const fn version<T>(number: u32) -> u32 {
    size_of::<T>() as u32 | (number << 16)
}

/// `NVDRS_PROFILE_V1`.
#[repr(C)]
#[allow(dead_code)] // Fields the driver reads or fills.
struct Profile1 {
    version: u32,
    name: [u16; UNICODE_MAX],
    gpu_support: u32,
    is_predefined: u32,
    applications: u32,
    settings: u32,
}
/// `NVDRS_APPLICATION_V1`.
#[repr(C)]
#[allow(dead_code)] // Fields the driver reads or fills.
struct Application1 {
    version: u32,
    is_predefined: u32,
    name: [u16; UNICODE_MAX],
    friendly_name: [u16; UNICODE_MAX],
    launcher: [u16; UNICODE_MAX],
}
/// `NVDRS_SETTING_V1`. Each value is a union of a DWORD, a 4100-byte binary
/// value and a string, so it is held as 1025 DWORDs.
#[repr(C)]
#[allow(dead_code)] // Fields the driver reads or fills.
struct Setting1 {
    version: u32,
    name: [u16; UNICODE_MAX],
    id: u32,
    kind: u32,
    location: u32,
    is_current_predefined: u32,
    is_predefined_valid: u32,
    predefined: [u32; 1025],
    current: [u32; 1025],
}

fn unicode(text: &str) -> [u16; UNICODE_MAX] {
    let mut out = [0u16; UNICODE_MAX];
    for (slot, unit) in out[..UNICODE_MAX - 1].iter_mut().zip(text.encode_utf16()) {
        *slot = unit;
    }
    out
}

fn zeroed<T>() -> Box<T> {
    // SAFETY: Only used for the plain-data NVAPI structures above, for which all-zero bytes are
    // a valid value.
    unsafe { Box::new(std::mem::zeroed()) }
}

pub(crate) type Handle = *mut c_void;
type QueryInterface = unsafe extern "C" fn(u32) -> *mut c_void;
type Status0 = unsafe extern "C" fn() -> i32;
type ErrorMessage = unsafe extern "C" fn(i32, *mut u8) -> i32;
type CreateSession = unsafe extern "C" fn(*mut Handle) -> i32;
type SessionCall = unsafe extern "C" fn(Handle) -> i32;
type FindProfile = unsafe extern "C" fn(Handle, *mut u16, *mut Handle) -> i32;
type CreateProfile = unsafe extern "C" fn(Handle, *mut Profile1, *mut Handle) -> i32;
type GetApplication = unsafe extern "C" fn(Handle, Handle, *mut u16, *mut Application1) -> i32;
type CreateApplication = unsafe extern "C" fn(Handle, Handle, *mut Application1) -> i32;
type GetSetting = unsafe extern "C" fn(Handle, Handle, u32, *mut Setting1) -> i32;
type SetSetting = unsafe extern "C" fn(Handle, Handle, *mut Setting1) -> i32;
type DeleteSetting = unsafe extern "C" fn(Handle, Handle, u32) -> i32;
type GlobalProfile = unsafe extern "C" fn(Handle, *mut Handle) -> i32;
type DriverVersion = unsafe extern "C" fn(*mut u32, *mut [u8; 64]) -> i32;

/// NVAPI's DRS entry points, from `nvapi64.dll`'s query interface.
pub(crate) struct NvApi {
    _library: libloading::Library,
    unload: Status0,
    error_message: Option<ErrorMessage>,
    destroy_session: SessionCall,
    save_settings: SessionCall,
    find_profile: FindProfile,
    create_profile: CreateProfile,
    get_application: GetApplication,
    create_application: CreateApplication,
    get_setting: GetSetting,
    set_setting: SetSetting,
    delete_setting: DeleteSetting,
    global_profile: Option<GlobalProfile>,
    driver_version: Option<DriverVersion>,
    session: Handle,
}

impl NvApi {
    /// Loads NVAPI and the driver settings. `None` when the PC has no NVIDIA
    /// driver or GPU.
    pub(crate) fn open() -> Result<Option<Self>> {
        let Ok(library) = crate::nvenc::system_library("nvapi64.dll") else {
            return Ok(None);
        };
        // SAFETY: The interface IDs are NVAPI's documented ones (nvapi_interface.h) and each
        // pointer is cast to that function's documented C signature. `library` moves into the
        // returned value, so the pointers stay loaded while it lives.
        unsafe {
            let query = *library
                .get::<QueryInterface>(b"nvapi_QueryInterface\0")
                .map_err(|error| anyhow!("nvapi64.dll has no query interface: {error}"))?;
            fn resolve<T: Copy>(query: QueryInterface, id: u32, name: &str) -> Result<T> {
                // SAFETY: `query` is NVAPI's own query interface.
                let pointer = unsafe { query(id) };
                if pointer.is_null() {
                    bail!("the NVIDIA driver does not provide {name}");
                }
                // SAFETY: `T` is a function pointer type of pointer size, chosen by the caller
                // for `id`.
                Ok(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&pointer) })
            }
            let initialize: Status0 = resolve(query, 0x0150_E828, "NvAPI_Initialize")?;
            let unload: Status0 = resolve(query, 0xD22B_DD7E, "NvAPI_Unload")?;
            let error_message: Option<ErrorMessage> =
                resolve(query, 0x6C2D_048C, "NvAPI_GetErrorMessage").ok();
            // Every entry point is resolved before NVAPI initialises, so a
            // missing one never leaves it initialised without an unload.
            let destroy_session = resolve(query, 0xDAD9_CFF8, "NvAPI_DRS_DestroySession")?;
            let save_settings = resolve(query, 0xFCBC_7E14, "NvAPI_DRS_SaveSettings")?;
            let find_profile = resolve(query, 0x7E4A_9A0B, "NvAPI_DRS_FindProfileByName")?;
            let create_profile = resolve(query, 0xCC17_6068, "NvAPI_DRS_CreateProfile")?;
            let get_application = resolve(query, 0xED1F_8C69, "NvAPI_DRS_GetApplicationInfo")?;
            let create_application = resolve(query, 0x4347_A9DE, "NvAPI_DRS_CreateApplication")?;
            let get_setting = resolve(query, 0x73BF_8338, "NvAPI_DRS_GetSetting")?;
            let set_setting = resolve(query, 0x577D_D202, "NvAPI_DRS_SetSetting")?;
            let delete_setting = resolve(query, 0xE4A2_6362, "NvAPI_DRS_DeleteProfileSetting")?;
            // Only the driver settings panel needs these two, so a driver without
            // them still gets the per-game profile.
            let global_profile: Option<GlobalProfile> =
                resolve(query, 0x617B_FF9F, "NvAPI_DRS_GetCurrentGlobalProfile").ok();
            let driver_version: Option<DriverVersion> =
                resolve(query, 0x2926_AAAD, "NvAPI_SYS_GetDriverAndBranchVersion").ok();
            let create: CreateSession = resolve(query, 0x0694_D52E, "NvAPI_DRS_CreateSession")?;
            let load: SessionCall = resolve(query, 0x375D_BD6B, "NvAPI_DRS_LoadSettings")?;
            match initialize() {
                OK => {}
                NVIDIA_DEVICE_NOT_FOUND | LIBRARY_NOT_FOUND => return Ok(None),
                status => bail!("NvAPI_Initialize failed with status {status}"),
            }
            let mut api = Self {
                unload,
                error_message,
                destroy_session,
                save_settings,
                find_profile,
                create_profile,
                get_application,
                create_application,
                get_setting,
                set_setting,
                delete_setting,
                global_profile,
                driver_version,
                session: std::ptr::null_mut(),
                _library: library,
            };
            let mut session = std::ptr::null_mut();
            let status = create(&mut session);
            api.session = session;
            api.check(status, "NvAPI_DRS_CreateSession")?;
            api.check(load(api.session), "NvAPI_DRS_LoadSettings")?;
            Ok(Some(api))
        }
    }

    fn check(&self, status: i32, operation: &str) -> Result<()> {
        if status == OK {
            return Ok(());
        }
        let mut text = [0u8; 64];
        let message = self
            .error_message
            // SAFETY: `text` is NVAPI's 64-byte short string, which it NUL-terminates.
            .filter(|get| unsafe { get(status, text.as_mut_ptr()) } == OK)
            .and_then(|_| std::ffi::CStr::from_bytes_until_nul(&text).ok())
            .map(|text| text.to_string_lossy().into_owned())
            .unwrap_or_default();
        bail!("{operation} failed with status {status} {message}")
    }

    /// The profile the NVIDIA Control Panel's Global Settings tab edits.
    pub(crate) fn global_profile(&self) -> Result<Handle> {
        let get = self
            .global_profile
            .ok_or_else(|| anyhow!("the NVIDIA driver has no global profile entry point"))?;
        let mut profile = std::ptr::null_mut();
        // SAFETY: `session` is this loaded NVAPI's open DRS session and `profile` a live
        // out-parameter.
        let status = unsafe { get(self.session, &mut profile) };
        self.check(status, "NvAPI_DRS_GetCurrentGlobalProfile")?;
        Ok(profile)
    }

    /// The driver version as NVIDIA numbers it, 571.86 as 57186.
    pub(crate) fn driver_version(&self) -> Option<u32> {
        let get = self.driver_version?;
        let mut version = 0u32;
        let mut branch = [0u8; 64];
        // SAFETY: Both are live out-parameters; `branch` is NVAPI's 64-byte short string.
        (unsafe { get(&mut version, &mut branch) } == OK).then_some(version)
    }
}

impl Drop for NvApi {
    fn drop(&mut self) {
        // SAFETY: `session` came from NvAPI_DRS_CreateSession on this loaded NVAPI, and NVAPI
        // is unloaded after it, once.
        unsafe {
            if !self.session.is_null() {
                (self.destroy_session)(self.session);
            }
            (self.unload)();
        }
    }
}

impl Drs for NvApi {
    type Profile = Handle;
    fn find_profile(&mut self, name: &str) -> Result<Option<Handle>> {
        let mut name = unicode(name);
        let mut profile = std::ptr::null_mut();
        // SAFETY: `name` is a full NvAPI_UnicodeString and `profile` a live out-parameter.
        match unsafe { (self.find_profile)(self.session, name.as_mut_ptr(), &mut profile) } {
            OK => Ok(Some(profile)),
            PROFILE_NOT_FOUND => Ok(None),
            status => self
                .check(status, "NvAPI_DRS_FindProfileByName")
                .map(|_| None),
        }
    }
    fn create_profile(&mut self, name: &str) -> Result<Handle> {
        let mut info = zeroed::<Profile1>();
        info.version = version::<Profile1>(1);
        info.name = unicode(name);
        let mut profile = std::ptr::null_mut();
        // SAFETY: `info` is a versioned NVDRS_PROFILE_V1 and `profile` a live out-parameter.
        let status = unsafe { (self.create_profile)(self.session, &mut *info, &mut profile) };
        self.check(status, "NvAPI_DRS_CreateProfile")?;
        Ok(profile)
    }
    fn has_application(&mut self, profile: Handle, name: &str) -> Result<bool> {
        let mut name = unicode(name);
        let mut info = zeroed::<Application1>();
        info.version = version::<Application1>(1);
        // SAFETY: `profile` came from this session, `name` is a full NvAPI_UnicodeString and
        // `info` a versioned NVDRS_APPLICATION_V1.
        let status =
            unsafe { (self.get_application)(self.session, profile, name.as_mut_ptr(), &mut *info) };
        match status {
            OK => Ok(true),
            EXECUTABLE_NOT_FOUND => Ok(false),
            status => self
                .check(status, "NvAPI_DRS_GetApplicationInfo")
                .map(|_| false),
        }
    }
    fn create_application(&mut self, profile: Handle, name: &str) -> Result<()> {
        let mut info = zeroed::<Application1>();
        info.version = version::<Application1>(1);
        info.name = unicode(name);
        info.friendly_name = unicode(name);
        // SAFETY: `profile` came from this session and `info` is a versioned
        // NVDRS_APPLICATION_V1.
        let status = unsafe { (self.create_application)(self.session, profile, &mut *info) };
        self.check(status, "NvAPI_DRS_CreateApplication")
    }
    fn setting(&mut self, profile: Handle, id: u32) -> Result<Option<Value>> {
        let mut setting = zeroed::<Setting1>();
        setting.version = version::<Setting1>(1);
        // SAFETY: `profile` came from this session and `setting` is a versioned NVDRS_SETTING_V1.
        let status = unsafe { (self.get_setting)(self.session, profile, id, &mut *setting) };
        match status {
            OK => Ok(Some(Value {
                value: setting.current[0],
                own: setting.location == CURRENT_PROFILE_LOCATION,
            })),
            SETTING_NOT_FOUND => Ok(None),
            status => self.check(status, "NvAPI_DRS_GetSetting").map(|_| None),
        }
    }
    fn set_setting(&mut self, profile: Handle, id: u32, value: u32) -> Result<()> {
        let mut setting = zeroed::<Setting1>();
        setting.version = version::<Setting1>(1);
        setting.id = id;
        setting.kind = DWORD_TYPE;
        setting.location = CURRENT_PROFILE_LOCATION;
        setting.current[0] = value;
        // SAFETY: `profile` came from this session and `setting` is a versioned DWORD
        // NVDRS_SETTING_V1.
        let status = unsafe { (self.set_setting)(self.session, profile, &mut *setting) };
        self.check(status, "NvAPI_DRS_SetSetting")
    }
    fn delete_setting(&mut self, profile: Handle, id: u32) -> Result<()> {
        // SAFETY: `profile` came from this session.
        match unsafe { (self.delete_setting)(self.session, profile, id) } {
            SETTING_NOT_FOUND => Ok(()),
            status => self.check(status, "NvAPI_DRS_DeleteProfileSetting"),
        }
    }
    fn save(&mut self) -> Result<()> {
        // SAFETY: `session` is this loaded NVAPI's open DRS session.
        let status = unsafe { (self.save_settings)(self.session) };
        self.check(status, "NvAPI_DRS_SaveSettings")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A driver settings store with one optional profile, as NVAPI keeps it.
    #[derive(Default)]
    struct Fake {
        profile: Option<FakeProfile>,
        saves: u32,
        calls: Vec<&'static str>,
    }
    #[derive(Default)]
    struct FakeProfile {
        applications: Vec<String>,
        pstate: Option<Value>,
    }
    impl Fake {
        fn with(applications: &[&str], pstate: Option<Value>) -> Self {
            Self {
                profile: Some(FakeProfile {
                    applications: applications.iter().map(|a| a.to_string()).collect(),
                    pstate,
                }),
                ..Self::default()
            }
        }
        fn profile(&mut self) -> &mut FakeProfile {
            self.profile.as_mut().unwrap()
        }
    }
    impl Drs for Fake {
        type Profile = ();
        fn find_profile(&mut self, name: &str) -> Result<Option<()>> {
            assert_eq!(name, PROFILE_NAME);
            Ok(self.profile.as_ref().map(|_| ()))
        }
        fn create_profile(&mut self, _: &str) -> Result<()> {
            self.calls.push("create_profile");
            self.profile = Some(FakeProfile::default());
            Ok(())
        }
        fn has_application(&mut self, _: (), name: &str) -> Result<bool> {
            Ok(self.profile().applications.iter().any(|a| a == name))
        }
        fn create_application(&mut self, _: (), name: &str) -> Result<()> {
            self.calls.push("create_application");
            self.profile().applications.push(name.into());
            Ok(())
        }
        fn setting(&mut self, _: (), id: u32) -> Result<Option<Value>> {
            assert_eq!(id, PREFERRED_PSTATE);
            Ok(self.profile().pstate)
        }
        fn set_setting(&mut self, _: (), id: u32, value: u32) -> Result<()> {
            assert_eq!(id, PREFERRED_PSTATE);
            self.calls.push("set_setting");
            self.profile().pstate = Some(Value { value, own: true });
            Ok(())
        }
        fn delete_setting(&mut self, _: (), _: u32) -> Result<()> {
            self.calls.push("delete_setting");
            self.profile().pstate = None;
            Ok(())
        }
        fn save(&mut self) -> Result<()> {
            self.saves += 1;
            Ok(())
        }
    }

    const MAX: Value = Value {
        value: PREFER_MAX,
        own: true,
    };

    #[test]
    fn a_fresh_driver_gets_the_profile_application_and_setting_saved_once() {
        let mut drs = Fake::default();
        assert_eq!(apply(&mut drs, true).unwrap(), Change::Set);
        assert_eq!(
            drs.calls,
            ["create_profile", "create_application", "set_setting"]
        );
        assert_eq!(drs.saves, 1);
        assert_eq!(drs.profile().applications, [APPLICATION]);
        assert_eq!(drs.profile().pstate, Some(MAX));
    }

    #[test]
    fn a_profile_already_set_is_left_alone_without_a_save() {
        let mut drs = Fake::with(&[APPLICATION], Some(MAX));
        assert_eq!(apply(&mut drs, true).unwrap(), Change::Unchanged);
        assert!(drs.calls.is_empty());
        assert_eq!(drs.saves, 0);
    }

    #[test]
    fn an_inherited_or_changed_power_setting_is_set_in_the_profile() {
        // Inherited from the global profile: the value alone is not ours.
        let inherited = Value {
            value: PREFER_MAX,
            own: false,
        };
        // Changed by the user in the NVIDIA app to optimal power.
        let changed = Value {
            value: 5,
            own: true,
        };
        for pstate in [inherited, changed] {
            let mut drs = Fake::with(&[APPLICATION], Some(pstate));
            assert_eq!(apply(&mut drs, true).unwrap(), Change::Set);
            assert_eq!(drs.calls, ["set_setting"]);
            assert_eq!(drs.profile().pstate, Some(MAX));
            assert_eq!(drs.saves, 1);
        }
    }

    #[test]
    fn a_profile_missing_the_host_gets_it_added() {
        let mut drs = Fake::with(&["other.exe"], Some(MAX));
        assert_eq!(apply(&mut drs, true).unwrap(), Change::Set);
        assert_eq!(drs.calls, ["create_application"]);
        assert_eq!(drs.profile().applications, ["other.exe", APPLICATION]);
        assert_eq!(drs.saves, 1);
    }

    #[test]
    fn turned_off_removes_only_the_profiles_own_power_setting() {
        let mut drs = Fake::with(&[APPLICATION], Some(MAX));
        assert_eq!(apply(&mut drs, false).unwrap(), Change::Removed);
        assert_eq!(drs.calls, ["delete_setting"]);
        assert_eq!(drs.profile().pstate, None);
        assert_eq!(drs.saves, 1);

        let inherited = Value {
            value: PREFER_MAX,
            own: false,
        };
        let mut drs = Fake::with(&[APPLICATION], Some(inherited));
        assert_eq!(apply(&mut drs, false).unwrap(), Change::Unchanged);
        assert_eq!(drs.saves, 0);
    }

    #[test]
    fn turned_off_never_creates_a_profile() {
        let mut drs = Fake::default();
        assert_eq!(apply(&mut drs, false).unwrap(), Change::Unchanged);
        assert!(drs.profile.is_none());
        assert!(drs.calls.is_empty());
        assert_eq!(drs.saves, 0);
    }

    #[test]
    fn structures_match_nvapi_v1_layouts() {
        // MAKE_NVAPI_VERSION(type, 1) = sizeof(type) | 1 << 16, from nvapi.h.
        assert_eq!(size_of::<Profile1>(), 4 + 4096 + 16);
        assert_eq!(size_of::<Application1>(), 8 + 3 * 4096);
        assert_eq!(size_of::<Setting1>(), 4 + 4096 + 20 + 2 * 4100);
        assert_eq!(version::<Profile1>(1), 0x0001_1014);
        assert_eq!(version::<Application1>(1), 0x0001_3008);
        assert_eq!(version::<Setting1>(1), 0x0001_3020);
        assert_eq!(std::mem::offset_of!(Setting1, id), 4100);
        assert_eq!(std::mem::offset_of!(Setting1, current), 4120 + 4100);
    }

    #[test]
    fn names_are_terminated_and_truncated() {
        let name = unicode(PROFILE_NAME);
        assert_eq!(String::from_utf16(&name[..9]).unwrap(), "Rubylight");
        assert_eq!(name[9], 0);
        let long = unicode(&"x".repeat(5000));
        assert_eq!(long[UNICODE_MAX - 1], 0);
    }

    #[test]
    fn no_nvidia_driver_is_skipped_quietly() {
        // This PC may or may not have NVAPI; the open must not fail just
        // because the library is absent.
        if crate::nvenc::system_library("nvapi64.dll").is_err() {
            assert!(NvApi::open().unwrap().is_none());
        }
    }
}
