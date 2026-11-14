//! MSVC Rust DLL adapting NVIDIA's NGX C ABI to the GNU Rust host. It is the
//! only code that links NVIDIA's NGX library, so it ships in Rubylight's
//! NVIDIA component, which the console downloads at the user's request, and
//! never in the signed installer. No C++ adapter is compiled. ABI definitions
//! are from RTX Video SDK 1.1.0: TrueHDR (RTX HDR) and VSR (RTX Video
//! enhancement), both through NVIDIA's documented NGX entry points.
//!
//! The feature DLLs (`nvngx_truehdr.dll`, `nvngx_vsr.dll`) sit in this DLL's
//! own folder and are named to NGX as its feature path; NGX's logs go to the
//! folder the host names with `butterpollo_ngx_configure`. The fixed feature
//! path, the absolute data path, the D3D multithread lock around every NGX
//! call (taken after this adapter's own lock, never before), the scratch
//! buffer check and the 400 to 2000 nit peak follow Foundation Sunshine's
//! TrueHDR adapter (AlkaidLab/foundation-sunshine, GPL-3.0,
//! src/platform/windows/image_enhancement/rtx_hdr/adapter/src/truehdr_adapter.cpp).
//!
//! ABI 2 added `butterpollo_ngx_available` and the VSR converter to ABI 1's
//! TrueHDR converter. ABI 3 adds `butterpollo_ngx_configure`, the minimum
//! driver version `butterpollo_ngx_available` reports, and requires that the
//! host holds no D3D multithread lock when it calls in.
use std::{
    collections::BTreeMap,
    ffi::{CStr, c_char, c_void},
    ptr,
    sync::Mutex,
};
use windows::{
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D11::*,
            Dxgi::{Common::*, IDXGIDevice},
        },
        System::LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            GetModuleFileNameW, GetModuleHandleExW,
        },
    },
    core::{Interface, PCWSTR},
};
// The declarations an application links against: the C branch of
// nvsdk_ngx.h without NGX_SNIPPET_BUILD. build.rs checks each one's parameter
// count against the SDK's headers.
unsafe extern "C" {
    /// `InFeatureInfo` comes before the version. The snippet-build form
    /// without it is the feature DLLs' own; called that way, NGX reads the
    /// version as the pointer.
    fn NVSDK_NGX_D3D11_Init(
        app: u64,
        path: *const u16,
        device: *mut c_void,
        feature_info: *const FeatureCommonInfo,
        version: u32,
    ) -> u32;
    fn NVSDK_NGX_D3D11_Shutdown1(device: *mut c_void) -> u32;
    fn NVSDK_NGX_D3D11_GetCapabilityParameters(params: *mut *mut c_void) -> u32;
    fn NVSDK_NGX_D3D11_DestroyParameters(params: *mut c_void) -> u32;
    fn NVSDK_NGX_D3D11_GetScratchBufferSize(
        feature: u32,
        params: *const c_void,
        size: *mut usize,
    ) -> u32;
    fn NVSDK_NGX_D3D11_CreateFeature(
        context: *mut c_void,
        feature: u32,
        params: *const c_void,
        handle: *mut *mut c_void,
    ) -> u32;
    fn NVSDK_NGX_D3D11_ReleaseFeature(handle: *mut c_void) -> u32;
    fn NVSDK_NGX_D3D11_EvaluateFeature_C(
        context: *mut c_void,
        handle: *const c_void,
        params: *const c_void,
        callback: *const c_void,
    ) -> u32;
    fn NVSDK_NGX_Parameter_GetI(params: *mut c_void, name: *const c_char, value: *mut i32) -> u32;
    fn NVSDK_NGX_Parameter_GetUI(params: *mut c_void, name: *const c_char, value: *mut u32) -> u32;
    fn NVSDK_NGX_Parameter_SetUI(params: *mut c_void, name: *const c_char, value: u32);
    fn NVSDK_NGX_Parameter_SetD3d11Resource(
        params: *mut c_void,
        name: *const c_char,
        value: *mut c_void,
    );
}
/// `NVSDK_NGX_PathListInfo` of nvsdk_ngx_defs.h.
#[repr(C)]
struct PathListInfo {
    path: *const *const u16,
    length: u32,
}
/// `NVSDK_NGX_LoggingInfo` of nvsdk_ngx_defs.h (SDK version 0x14 and later).
#[repr(C)]
struct LoggingInfo {
    callback: *const c_void,
    /// `NVSDK_NGX_Logging_Level`; 0 is `NVSDK_NGX_LOGGING_LEVEL_OFF`.
    minimum_level: i32,
    disable_other_sinks: bool,
}
/// `NVSDK_NGX_FeatureCommonInfo` of nvsdk_ngx_defs.h.
#[repr(C)]
struct FeatureCommonInfo {
    path_list: PathListInfo,
    internal: *mut c_void,
    logging: LoggingInfo,
}
/// `NVSDK_NGX_Version_API` of the SDK's headers.
const NGX_VERSION: u32 = 0x15;
// `NGX_APPLICATION_ID`: NVIDIA's application ID for Rubylight, from the build's
// `RUBYLIGHT_NGX_APPLICATION_ID`; 0, the SDK's default, without one.
include!(concat!(env!("OUT_DIR"), "/application_id.rs"));
/// NVIDIA's PCI vendor id.
const NVIDIA: u32 = 0x10de;
/// Errors of this adapter, apart from NGX's own result codes (0xbad0_00xx).
const ERROR_NULL_PARAMETERS: u32 = 0xbad1_0001;
const ERROR_FORMAT: u32 = 0xbad1_0002;
const ERROR_UNAVAILABLE: u32 = 0xbad1_0003;
const ERROR_FEATURE: u32 = 0xbad1_0004;
/// The host has not named NGX's data folder (`butterpollo_ngx_configure`).
const ERROR_UNCONFIGURED: u32 = 0xbad1_0005;
/// The driver is older than the feature needs; the minimum is reported.
const ERROR_DRIVER: u32 = 0xbad1_0006;
/// The feature asks for scratch memory, which this D3D11 path never gives.
const ERROR_SCRATCH: u32 = 0xbad1_0007;
/// The GPU cannot bind the output's format as a typed UAV, which NGX writes.
/// An ABI 3 host without this code shows it as a plain NGX error number.
const ERROR_OUTPUT_UAV: u32 = 0xbad1_0008;

/// The two NGX features this adapter runs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Feature {
    /// `NVSDK_NGX_Feature_TrueHDR` (Reserved14).
    TrueHdr,
    /// `NVSDK_NGX_Feature_VSR` (Reserved16).
    Vsr,
}
impl Feature {
    fn from_abi(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::TrueHdr),
            1 => Some(Self::Vsr),
            _ => None,
        }
    }
    fn id(self) -> u32 {
        match self {
            Self::TrueHdr => 14,
            Self::Vsr => 16,
        }
    }
    /// The capability parameters: available, needs an updated driver, the
    /// minimum driver's major and minor version, and the init result.
    fn capability(self) -> [&'static CStr; 5] {
        match self {
            Self::TrueHdr => [
                c"TrueHDR.Available",
                c"TrueHDR.NeedsUpdatedDriver",
                c"TrueHDR.MinDriverVersionMajor",
                c"TrueHDR.MinDriverVersionMinor",
                c"TrueHDR.FeatureInitResult",
            ],
            Self::Vsr => [
                c"VSR.Available",
                c"VSR.NeedsUpdatedDriver",
                c"VSR.MinDriverVersionMajor",
                c"VSR.MinDriverVersionMinor",
                c"VSR.FeatureInitResult",
            ],
        }
    }
}

/// A device's NGX instance and how many use it. The device type is a
/// parameter only so the tests can count without a GPU.
struct Runtime<D = ID3D11Device> {
    device: D,
    users: usize,
}
/// NGX is initialized once per D3D11 device and shut down when its last user
/// goes. NGX is not thread safe: every call holds this lock, and then the
/// device's D3D multithread lock, in that order only.
static DEVICES: Mutex<BTreeMap<usize, Runtime>> = Mutex::new(BTreeMap::new());
/// NGX's data folder (its logs), NUL-terminated, from the host.
static DATA_PATH: Mutex<Option<Vec<u16>>> = Mutex::new(None);
fn check(code: u32) -> Result<(), u32> {
    if code == 1 { Ok(()) } else { Err(code) }
}
/// A device's D3D multithread lock, held while this lives. The adapter
/// takes it only while it holds `DEVICES`, so the two are always taken in
/// the same order.
struct Locked(Option<ID3D11Multithread>);
impl Locked {
    fn new(device: &ID3D11Device) -> Self {
        // SAFETY: `device` is a live D3D11 device; Enter is balanced by Drop.
        unsafe {
            let lock = device
                .GetImmediateContext()
                .ok()
                .and_then(|context| context.cast::<ID3D11Multithread>().ok());
            if let Some(lock) = &lock {
                let _ = lock.SetMultithreadProtected(true);
                lock.Enter();
            }
            Self(lock)
        }
    }
}
impl Drop for Locked {
    fn drop(&mut self) {
        if let Some(lock) = &self.0 {
            // SAFETY: entered in `new`, on this thread.
            unsafe { lock.Leave() };
        }
    }
}
/// Refuses another vendor's adapter before entering NGX.
fn require_nvidia(device: &ID3D11Device) -> Result<(), u32> {
    // SAFETY: `device` is a live D3D11 device; the DXGI calls only read its adapter.
    let adapter = unsafe {
        device
            .cast::<IDXGIDevice>()
            .map_err(|e| e.code().0 as u32)?
            .GetAdapter()
            .map_err(|e| e.code().0 as u32)?
            .GetDesc()
            .map_err(|e| e.code().0 as u32)?
    };
    if adapter.VendorId != NVIDIA {
        return Err(ERROR_UNAVAILABLE);
    }
    Ok(())
}
/// This DLL's folder, NUL-terminated: where the NVIDIA component keeps the
/// feature DLLs beside it.
fn own_folder() -> Result<Vec<u16>, u32> {
    let mut module = HMODULE::default();
    // SAFETY: the address is a function of this module, and the handle is not
    // counted, so nothing is released.
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(own_folder as *const () as *const u16),
            &mut module,
        )
        .map_err(|e| e.code().0 as u32)?;
    }
    let mut buffer = vec![0u16; 32768];
    // SAFETY: `module` is this loaded module and `buffer` a writable slice.
    let length = unsafe { GetModuleFileNameW(Some(module), &mut buffer) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(ERROR_UNAVAILABLE);
    }
    buffer.truncate(length);
    let slash = buffer
        .iter()
        .rposition(|&c| c == b'\\' as u16 || c == b'/' as u16)
        .ok_or(ERROR_UNAVAILABLE)?;
    buffer.truncate(slash);
    buffer.push(0);
    Ok(buffer)
}
/// Counts one more user of NGX on `device`, initializing it for the first.
/// The caller holds `devices` and the device's multithread lock.
fn acquire(devices: &mut BTreeMap<usize, Runtime>, device: &ID3D11Device) -> Result<(), u32> {
    let id = device.as_raw() as usize;
    if let Some(runtime) = devices.get_mut(&id) {
        runtime.users += 1;
        return Ok(());
    }
    let data = DATA_PATH
        .lock()
        .unwrap()
        .clone()
        .ok_or(ERROR_UNCONFIGURED)?;
    let folder = own_folder()?;
    let paths = [folder.as_ptr()];
    let info = FeatureCommonInfo {
        path_list: PathListInfo {
            path: paths.as_ptr(),
            length: 1,
        },
        internal: ptr::null_mut(),
        logging: LoggingInfo {
            callback: ptr::null(),
            minimum_level: 0,
            disable_other_sinks: false,
        },
    };
    // SAFETY: `device` is a live D3D11 device, `data` and `folder` are
    // NUL-terminated wide strings and `info` a complete FeatureCommonInfo,
    // all alive for the call.
    check(unsafe {
        NVSDK_NGX_D3D11_Init(
            NGX_APPLICATION_ID,
            data.as_ptr(),
            device.as_raw(),
            &info,
            NGX_VERSION,
        )
    })?;
    devices.insert(
        id,
        Runtime {
            device: device.clone(),
            users: 1,
        },
    );
    Ok(())
}
/// Counts one user of the device `id` gone, and takes its entry out of
/// `devices` when that was its last: the device to shut down. Other devices
/// keep theirs, whatever their counts.
fn count_down<D>(devices: &mut BTreeMap<usize, Runtime<D>>, id: usize) -> Option<Runtime<D>> {
    let runtime = devices.get_mut(&id)?;
    runtime.users = runtime.users.saturating_sub(1);
    if runtime.users == 0 {
        devices.remove(&id)
    } else {
        None
    }
}
/// Counts one user of NGX on `device` gone. After its last, NGX shuts down on
/// that device alone ("Shutdown1(Device) only affects specified device",
/// nvsdk_ngx.h), and its entry is returned to be released outside the lock: a
/// final D3D device release can wait for display removal. The caller holds
/// `devices` and `device`'s multithread lock.
fn release(devices: &mut BTreeMap<usize, Runtime>, device: &ID3D11Device) -> Option<Runtime> {
    let runtime = count_down(devices, device.as_raw() as usize)?;
    // SAFETY: NGX was initialized on this live device, and no feature on it remains.
    unsafe {
        NVSDK_NGX_D3D11_Shutdown1(runtime.device.as_raw());
    }
    Some(runtime)
}
/// The error for an output texture D3D would not create: `ERROR_OUTPUT_UAV`
/// when the device's format support (`support`, if it could be read) lacks a
/// typed UAV, otherwise D3D's own result.
fn output_failure(support: Option<u32>, hresult: u32) -> u32 {
    match support {
        Some(bits) if (bits & D3D11_FORMAT_SUPPORT_TYPED_UNORDERED_ACCESS_VIEW.0 as u32) == 0 => {
            ERROR_OUTPUT_UAV
        }
        _ => hresult,
    }
}
/// The minimum driver a feature asks for, as major << 16 | minor.
fn driver_version(major: u32, minor: u32) -> u32 {
    (major.min(0xffff) << 16) | minor.min(0xffff)
}
/// NGX's capability parameters when `feature` is available. When it is not,
/// the reason: an older driver than it needs (with `min_driver` set), NGX's
/// own init result, or plain unavailability. The caller holds the locks and
/// has acquired NGX on the device.
unsafe fn feature_available(feature: Feature, min_driver: &mut u32) -> Result<*mut c_void, u32> {
    unsafe {
        let mut params = ptr::null_mut();
        check(NVSDK_NGX_D3D11_GetCapabilityParameters(&mut params))?;
        if params.is_null() {
            return Err(ERROR_NULL_PARAMETERS);
        }
        let [available, needs_driver, major, minor, init] = feature.capability();
        let mut value = 0;
        let result = check(NVSDK_NGX_Parameter_GetI(
            params,
            available.as_ptr(),
            &mut value,
        ));
        if result.is_ok() && value != 0 {
            return Ok(params);
        }
        let mut reason = result.err().unwrap_or(ERROR_UNAVAILABLE);
        let mut needs = 0;
        if check(NVSDK_NGX_Parameter_GetI(
            params,
            needs_driver.as_ptr(),
            &mut needs,
        ))
        .is_ok()
            && needs != 0
        {
            let (mut high, mut low) = (0u32, 0u32);
            let _ = NVSDK_NGX_Parameter_GetUI(params, major.as_ptr(), &mut high);
            let _ = NVSDK_NGX_Parameter_GetUI(params, minor.as_ptr(), &mut low);
            *min_driver = driver_version(high, low);
            reason = ERROR_DRIVER;
        } else {
            let mut code = 0;
            // A failed init reports NGX's own result code (0xbadxxxxx).
            if check(NVSDK_NGX_Parameter_GetI(params, init.as_ptr(), &mut code)).is_ok()
                && (code as u32) & 0xfff0_0000 == 0xbad0_0000
            {
                reason = code as u32;
            }
        }
        NVSDK_NGX_D3D11_DestroyParameters(params);
        Err(reason)
    }
}

/// One NGX feature on one device: its parameters, its handle and the output
/// texture it writes, which stays owned here.
struct Converter {
    feature: Feature,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    params: *mut c_void,
    handle: *mut c_void,
    output: Option<ID3D11Texture2D>,
    /// Width, height and format of `output`.
    shape: (u32, u32, DXGI_FORMAT),
    acquired: bool,
}
impl Converter {
    unsafe fn new(device: ID3D11Device, feature: Feature) -> Result<Box<Self>, u32> {
        unsafe {
            require_nvidia(&device)?;
            let context = device
                .GetImmediateContext()
                .map_err(|e| e.code().0 as u32)?;
            let mut state = Box::new(Self {
                feature,
                device,
                context,
                params: ptr::null_mut(),
                handle: ptr::null_mut(),
                output: None,
                shape: (0, 0, DXGI_FORMAT_UNKNOWN),
                acquired: false,
            });
            // On failure, Drop releases what was made, after these locks go.
            let mut devices = DEVICES.lock().unwrap();
            let locked = Locked::new(&state.device);
            acquire(&mut devices, &state.device)?;
            state.acquired = true;
            state.params = feature_available(feature, &mut 0)?;
            // TrueHDR asks for no scratch memory, and this adapter gives none:
            // one that asks is refused before it is created.
            if matches!(feature, Feature::TrueHdr) {
                let mut scratch = 0;
                check(NVSDK_NGX_D3D11_GetScratchBufferSize(
                    feature.id(),
                    state.params,
                    &mut scratch,
                ))?;
                if scratch != 0 {
                    return Err(ERROR_SCRATCH);
                }
            }
            check(NVSDK_NGX_D3D11_CreateFeature(
                state.context.as_raw(),
                feature.id(),
                state.params,
                &mut state.handle,
            ))?;
            if state.handle.is_null() {
                return Err(ERROR_FEATURE);
            }
            drop(locked);
            drop(devices);
            Ok(state)
        }
    }
    /// The output texture at the input's size in `format`, made again when either changes.
    unsafe fn output(
        &mut self,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
    ) -> Result<*mut c_void, u32> {
        unsafe {
            if self.shape != (width, height, format) || self.output.is_none() {
                self.output = None;
                // NGX writes its destination as an unordered access view: "DX11 and DX12
                // destination resources must be UAV" (the SDK guide, chapter 3), and NVIDIA's
                // samples evaluate into a UAV texture of their own when the caller's has none.
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: width,
                    Height: height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: format,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: (D3D11_BIND_SHADER_RESOURCE.0
                        | D3D11_BIND_RENDER_TARGET.0
                        | D3D11_BIND_UNORDERED_ACCESS.0) as u32,
                    ..Default::default()
                };
                if let Err(error) = self
                    .device
                    .CreateTexture2D(&desc, None, Some(&mut self.output))
                {
                    // A texture without UAV would fail every evaluation, so none is made.
                    self.output = None;
                    let support = self.device.CheckFormatSupport(format).ok();
                    return Err(output_failure(support, error.code().0 as u32));
                }
                self.shape = (width, height, format);
            }
            Ok(self.output.as_ref().ok_or(ERROR_FEATURE)?.as_raw())
        }
    }
    /// The input's size after checking it and its format, which `formats` lists.
    unsafe fn input(
        input: &ID3D11Texture2D,
        formats: &[DXGI_FORMAT],
    ) -> Result<D3D11_TEXTURE2D_DESC, u32> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `input` is a live texture.
        unsafe { input.GetDesc(&mut desc) };
        if !formats.contains(&desc.Format)
            || desc.Width == 0
            || desc.Height == 0
            || desc.Width > 16384
            || desc.Height > 16384
        {
            return Err(ERROR_FORMAT);
        }
        Ok(desc)
    }
    /// Runs the feature. The caller holds `DEVICES` and the device's lock.
    unsafe fn evaluate(
        &mut self,
        input: &ID3D11Texture2D,
        output: *mut c_void,
        values: &[(&CStr, u32)],
    ) -> Result<*mut c_void, u32> {
        unsafe {
            NVSDK_NGX_Parameter_SetD3d11Resource(self.params, c"Input1".as_ptr(), input.as_raw());
            NVSDK_NGX_Parameter_SetD3d11Resource(self.params, c"Output".as_ptr(), output);
            for (name, value) in values {
                NVSDK_NGX_Parameter_SetUI(self.params, name.as_ptr(), *value);
            }
            check(NVSDK_NGX_D3D11_EvaluateFeature_C(
                self.context.as_raw(),
                self.handle,
                self.params,
                ptr::null(),
            ))?;
            Ok(output)
        }
    }
    unsafe fn convert_truehdr(
        &mut self,
        input: &ID3D11Texture2D,
        contrast: u32,
        saturation: u32,
        middle: u32,
        peak: u32,
    ) -> Result<*mut c_void, u32> {
        unsafe {
            let _devices = DEVICES.lock().unwrap();
            let _locked = Locked::new(&self.device);
            if self.feature != Feature::TrueHdr {
                return Err(ERROR_FEATURE);
            }
            let desc = Self::input(
                input,
                &[
                    DXGI_FORMAT_B8G8R8A8_UNORM,
                    DXGI_FORMAT_R8G8B8A8_UNORM,
                    DXGI_FORMAT_R10G10B10A2_UNORM,
                ],
            )?;
            let output = self.output(desc.Width, desc.Height, DXGI_FORMAT_R16G16B16A16_FLOAT)?;
            self.evaluate(
                input,
                output,
                &[
                    (c"TrueHDR.InLeft", 0),
                    (c"TrueHDR.InTop", 0),
                    (c"TrueHDR.InRight", desc.Width),
                    (c"TrueHDR.InBottom", desc.Height),
                    (c"TrueHDR.OutLeft", 0),
                    (c"TrueHDR.OutTop", 0),
                    (c"TrueHDR.OutRight", desc.Width),
                    (c"TrueHDR.OutBottom", desc.Height),
                    (c"TrueHDR.Contrast", contrast.min(200)),
                    (c"TrueHDR.Saturation", saturation.min(200)),
                    (c"TrueHDR.MiddleGray", middle.clamp(10, 100)),
                    // nvsdk_ngx_defs_truehdr.h: 400 to 2000 for the monitor's peak.
                    (c"TrueHDR.MaxLuminance", peak.clamp(400, 2000)),
                ],
            )
        }
    }
    /// VSR with the output rectangle the input's own: the SDK's upscale,
    /// sharpen and deblock at 1:1.
    unsafe fn convert_vsr(
        &mut self,
        input: &ID3D11Texture2D,
        quality: u32,
    ) -> Result<*mut c_void, u32> {
        unsafe {
            let _devices = DEVICES.lock().unwrap();
            let _locked = Locked::new(&self.device);
            if self.feature != Feature::Vsr {
                return Err(ERROR_FEATURE);
            }
            let desc = Self::input(
                input,
                &[DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM],
            )?;
            let output = self.output(desc.Width, desc.Height, desc.Format)?;
            self.evaluate(
                input,
                output,
                &[
                    (c"Rect.X", 0),
                    (c"Rect.Y", 0),
                    (c"Rect.W", desc.Width),
                    (c"Rect.H", desc.Height),
                    (c"OutRect.X", 0),
                    (c"OutRect.Y", 0),
                    (c"OutRect.W", desc.Width),
                    (c"OutRect.H", desc.Height),
                    // NVSDK_NGX_VSR_Quality_Low (1) to _Ultra (4).
                    (c"VSR.QualityLevel", quality.clamp(1, 4)),
                ],
            )
        }
    }
}
impl Drop for Converter {
    fn drop(&mut self) {
        self.output = None;
        let releases = {
            let mut devices = DEVICES.lock().unwrap();
            let _locked = Locked::new(&self.device);
            // SAFETY: the handle and parameters came from NGX on this device and are released once.
            unsafe {
                if !self.handle.is_null() {
                    NVSDK_NGX_D3D11_ReleaseFeature(self.handle);
                }
                if !self.params.is_null() {
                    NVSDK_NGX_D3D11_DestroyParameters(self.params);
                }
            }
            if self.acquired {
                release(&mut devices, &self.device)
            } else {
                None
            }
        };
        // Never hold the NGX lock while a device goes.
        drop(releases);
    }
}

fn report(error: *mut u32, code: u32) {
    if !error.is_null() {
        // SAFETY: callers pass null or a writable u32.
        unsafe { *error = code };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn butterpollo_truehdr_abi() -> u32 {
    3
}
/// Names NGX's data folder (its logs): an absolute, NUL-terminated wide
/// path that exists. Call once before anything else; 1 if taken, 0 if not.
/// # Safety
/// `path` must be null or a NUL-terminated wide string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn butterpollo_ngx_configure(path: *const u16) -> u32 {
    if path.is_null() {
        return 0;
    }
    // SAFETY: the caller passes a NUL-terminated wide string.
    let wide = PCWSTR(path);
    let text = unsafe { wide.as_wide() }.to_vec();
    // A drive (C:\...) or a UNC path (\\...): never the current folder,
    // which is System32 for the service.
    let absolute = (text.len() >= 3 && text[1] == b':' as u16 && text[2] == b'\\' as u16)
        || text.starts_with(&[b'\\' as u16, b'\\' as u16]);
    if !absolute {
        return 0;
    }
    let mut text = text;
    text.push(0);
    *DATA_PATH.lock().unwrap() = Some(text);
    1
}
/// Whether `feature` (0 TrueHDR, 1 VSR) is available on `device`: 1 if so, 0
/// with `error` set otherwise, and `min_driver` (major << 16 | minor) set when
/// the driver is older than the feature needs. NGX is initialized for the
/// check and shut down after it unless a converter still uses it.
/// # Safety
/// `device` must be null or a valid ID3D11Device interface; a non-null `error`
/// or `min_driver` must point to a writable u32. The caller holds no D3D
/// multithread lock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn butterpollo_ngx_available(
    device: *mut c_void,
    feature: u32,
    error: *mut u32,
    min_driver: *mut u32,
) -> u32 {
    report(error, 0);
    report(min_driver, 0);
    // SAFETY: the caller passes null or a live device.
    let Some(device) = (unsafe { ID3D11Device::from_raw_borrowed(&device) }) else {
        report(error, ERROR_NULL_PARAMETERS);
        return 0;
    };
    let Some(feature) = Feature::from_abi(feature) else {
        report(error, ERROR_FEATURE);
        return 0;
    };
    if let Err(code) = require_nvidia(device) {
        report(error, code);
        return 0;
    }
    let mut minimum = 0;
    let (result, releases) = {
        let mut devices = DEVICES.lock().unwrap();
        let _locked = Locked::new(device);
        match acquire(&mut devices, device) {
            Err(code) => (Err(code), None),
            Ok(()) => {
                // SAFETY: the locks are held and NGX is acquired on `device`.
                let result = unsafe { feature_available(feature, &mut minimum) }.map(|params| {
                    // SAFETY: `params` came from GetCapabilityParameters.
                    unsafe { NVSDK_NGX_D3D11_DestroyParameters(params) };
                });
                (result, release(&mut devices, device))
            }
        }
    };
    drop(releases);
    report(min_driver, minimum);
    match result {
        Ok(()) => 1,
        Err(code) => {
            report(error, code);
            0
        }
    }
}
unsafe fn create(device: *mut c_void, feature: Feature, error: *mut u32) -> *mut c_void {
    report(error, 0);
    // SAFETY: the caller passes null or a live device.
    let Some(device) = (unsafe { ID3D11Device::from_raw_borrowed(&device) }) else {
        return ptr::null_mut();
    };
    // SAFETY: `device` is live; the converter keeps its own reference.
    match unsafe { Converter::new(device.clone(), feature) } {
        Ok(state) => Box::into_raw(state).cast(),
        Err(code) => {
            report(error, code);
            ptr::null_mut()
        }
    }
}
fn finish(result: Result<*mut c_void, u32>, error: *mut u32) -> *mut c_void {
    match result {
        Ok(out) => out,
        Err(code) => {
            report(error, code);
            ptr::null_mut()
        }
    }
}
/// Create a TrueHDR converter borrowing a D3D11 device for the duration of the call.
/// # Safety
/// `device` must be null or a valid ID3D11Device interface; a non-null `error`
/// must point to a writable u32. Destroy the returned state exactly once. The
/// caller holds no D3D multithread lock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn butterpollo_truehdr_create(
    device: *mut c_void,
    error: *mut u32,
) -> *mut c_void {
    unsafe { create(device, Feature::TrueHdr, error) }
}
/// Convert an SDR texture; the returned texture remains owned by the state.
/// # Safety
/// `state` must be null or a live state returned by create; `input` must be
/// null or a valid ID3D11Texture2D on its device. A non-null `error` must be
/// writable. Calls on one state must be serialized and use its owning thread,
/// which holds no D3D multithread lock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn butterpollo_truehdr_convert(
    state: *mut c_void,
    input: *mut c_void,
    contrast: u32,
    saturation: u32,
    middle: u32,
    peak: u32,
    error: *mut u32,
) -> *mut c_void {
    report(error, 0);
    unsafe {
        let Some(state) = state.cast::<Converter>().as_mut() else {
            return ptr::null_mut();
        };
        let Some(input) = ID3D11Texture2D::from_raw_borrowed(&input) else {
            return ptr::null_mut();
        };
        finish(
            state.convert_truehdr(input, contrast, saturation, middle, peak),
            error,
        )
    }
}
/// Release a converter and its feature resources.
/// # Safety
/// `state` must be null or a state returned by a create function, with no
/// concurrent calls; a non-null state must be destroyed exactly once on its
/// owning thread, which holds no D3D multithread lock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn butterpollo_truehdr_destroy(state: *mut c_void) {
    if !state.is_null() {
        unsafe {
            drop(Box::from_raw(state.cast::<Converter>()));
        }
    }
}
/// Create a VSR converter (RTX Video enhancement) on a D3D11 device.
/// # Safety
/// As [`butterpollo_truehdr_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn butterpollo_vsr_create(
    device: *mut c_void,
    error: *mut u32,
) -> *mut c_void {
    unsafe { create(device, Feature::Vsr, error) }
}
/// Enhance an 8-bit SDR texture at its own size with VSR quality level
/// `quality` (1 to 4); the returned texture, in the input's format, remains
/// owned by the state.
/// # Safety
/// As [`butterpollo_truehdr_convert`], for a state from [`butterpollo_vsr_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn butterpollo_vsr_convert(
    state: *mut c_void,
    input: *mut c_void,
    quality: u32,
    error: *mut u32,
) -> *mut c_void {
    report(error, 0);
    unsafe {
        let Some(state) = state.cast::<Converter>().as_mut() else {
            return ptr::null_mut();
        };
        let Some(input) = ID3D11Texture2D::from_raw_borrowed(&input) else {
            return ptr::null_mut();
        };
        finish(state.convert_vsr(input, quality), error)
    }
}
/// Release a VSR converter.
/// # Safety
/// As [`butterpollo_truehdr_destroy`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn butterpollo_vsr_destroy(state: *mut c_void) {
    unsafe { butterpollo_truehdr_destroy(state) }
}

#[cfg(test)]
mod tests {
    use super::{BTreeMap, Runtime, count_down};

    #[test]
    fn the_minimum_driver_packs_major_over_minor() {
        assert_eq!(super::driver_version(570, 2), (570 << 16) | 2);
        assert_eq!(super::driver_version(0x1_0000, 0x1_0000), 0xffff_ffff);
    }

    fn devices(entries: &[(usize, usize)]) -> BTreeMap<usize, Runtime<()>> {
        entries
            .iter()
            .map(|&(id, users)| (id, Runtime { device: (), users }))
            .collect()
    }
    fn users(devices: &BTreeMap<usize, Runtime<()>>) -> Vec<(usize, usize)> {
        devices.iter().map(|(&id, d)| (id, d.users)).collect()
    }

    /// A device's last user shuts that device down even while another
    /// device is still in use, and leaves the other one alone.
    #[test]
    fn a_device_shuts_down_after_its_own_last_user() {
        let mut table = devices(&[(1, 1), (2, 3)]);
        assert_eq!(count_down(&mut table, 1).map(|d| d.users), Some(0));
        assert_eq!(users(&table), [(2, 3)]);
    }

    #[test]
    fn a_device_with_users_left_stays() {
        let mut table = devices(&[(1, 2), (2, 1)]);
        assert!(count_down(&mut table, 1).is_none());
        assert_eq!(users(&table), [(1, 1), (2, 1)]);
    }

    #[test]
    fn an_unknown_device_changes_nothing() {
        let mut table = devices(&[(1, 1)]);
        assert!(count_down(&mut table, 9).is_none());
        assert_eq!(users(&table), [(1, 1)]);
    }

    /// One device, as before: it shuts down after its last user and the table
    /// is empty.
    #[test]
    fn a_single_device_shuts_down_after_its_last_user() {
        let mut table = devices(&[(1, 2)]);
        assert!(count_down(&mut table, 1).is_none());
        assert!(count_down(&mut table, 1).is_some());
        assert!(table.is_empty());
    }

    /// The output must be a UAV for NGX; a format the GPU cannot bind as one
    /// gets its own error, and any other failure keeps D3D's result.
    #[test]
    fn an_output_without_typed_uav_gets_its_own_error() {
        use super::{D3D11_FORMAT_SUPPORT_TYPED_UNORDERED_ACCESS_VIEW, ERROR_OUTPUT_UAV};
        let uav = D3D11_FORMAT_SUPPORT_TYPED_UNORDERED_ACCESS_VIEW.0 as u32;
        let out_of_memory = 0x8007_000e;
        assert_eq!(
            super::output_failure(Some(0), out_of_memory),
            ERROR_OUTPUT_UAV
        );
        assert_eq!(
            super::output_failure(Some(uav), out_of_memory),
            out_of_memory
        );
        assert_eq!(super::output_failure(None, out_of_memory), out_of_memory);
    }
}
