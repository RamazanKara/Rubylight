//! MSVC Rust DLL adapting NVIDIA's NGX C ABI to the GNU Rust host, for
//! Rubylight's separate, unsigned NVIDIA build only. No C++ adapter is
//! compiled. ABI definitions are from RTX Video SDK 1.1.0: TrueHDR (RTX HDR)
//! and VSR (RTX Video enhancement), both through NVIDIA's documented NGX
//! entry points, with the feature DLLs (`nvngx_truehdr.dll`, `nvngx_vsr.dll`)
//! beside the host as the SDK's guide describes.
//!
//! ABI 2 adds `butterpollo_ngx_available` and the VSR converter to ABI 1's
//! TrueHDR converter.
use std::{
    collections::BTreeMap,
    ffi::{CStr, c_char, c_void},
    ptr,
    sync::Mutex,
};
use windows::{
    Win32::Graphics::{
        Direct3D11::*,
        Dxgi::{Common::*, IDXGIDevice},
    },
    core::Interface,
};
// The declarations an application links against: the C branch of
// nvsdk_ngx.h without NGX_SNIPPET_BUILD. build.rs checks each one's parameter
// count against the SDK's headers.
unsafe extern "C" {
    /// `InFeatureInfo` (null: the feature DLLs beside the executable) comes
    /// before the version. The snippet-build form without it is the feature
    /// DLLs' own; called that way, NGX reads the version as the pointer.
    fn NVSDK_NGX_D3D11_Init(
        app: u64,
        path: *const u16,
        device: *mut c_void,
        feature_info: *const c_void,
        version: u32,
    ) -> u32;
    fn NVSDK_NGX_D3D11_Shutdown1(device: *mut c_void) -> u32;
    fn NVSDK_NGX_D3D11_GetCapabilityParameters(params: *mut *mut c_void) -> u32;
    fn NVSDK_NGX_D3D11_DestroyParameters(params: *mut c_void) -> u32;
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
    fn NVSDK_NGX_Parameter_SetUI(params: *mut c_void, name: *const c_char, value: u32);
    fn NVSDK_NGX_Parameter_SetD3d11Resource(
        params: *mut c_void,
        name: *const c_char,
        value: *mut c_void,
    );
}
/// `NVSDK_NGX_Version_API` of the SDK's headers.
const NGX_VERSION: u32 = 0x15;
/// NVIDIA's PCI vendor id.
const NVIDIA: u32 = 0x10de;
/// Errors of this adapter, apart from NGX's own result codes (0xbad0_00xx).
const ERROR_NULL_PARAMETERS: u32 = 0xbad1_0001;
const ERROR_FORMAT: u32 = 0xbad1_0002;
const ERROR_UNAVAILABLE: u32 = 0xbad1_0003;
const ERROR_FEATURE: u32 = 0xbad1_0004;

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
    fn available(self) -> &'static CStr {
        match self {
            Self::TrueHdr => c"TrueHDR.Available",
            Self::Vsr => c"VSR.Available",
        }
    }
}

struct Runtime {
    device: ID3D11Device,
    users: usize,
}
/// NGX is initialized once per D3D11 device and shut down when its last user
/// goes. NGX is not thread safe: every call holds this lock.
static DEVICES: Mutex<BTreeMap<usize, Runtime>> = Mutex::new(BTreeMap::new());
fn check(code: u32) -> Result<(), u32> {
    if code == 1 { Ok(()) } else { Err(code) }
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
/// Counts one more user of NGX on `device`, initializing it for the first.
/// The caller holds `devices`.
fn acquire(devices: &mut BTreeMap<usize, Runtime>, device: &ID3D11Device) -> Result<(), u32> {
    let id = device.as_raw() as usize;
    if let Some(runtime) = devices.get_mut(&id) {
        runtime.users += 1;
        return Ok(());
    }
    // The feature DLLs and NGX's logs: the executable's folder, as the
    // SDK's guide describes.
    let path = [b'.' as u16, 0];
    // SAFETY: `device` is a live D3D11 device, `path` a NUL-terminated wide
    // string, and a null feature info is the SDK's default.
    check(unsafe {
        NVSDK_NGX_D3D11_Init(0, path.as_ptr(), device.as_raw(), ptr::null(), NGX_VERSION)
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
/// Counts one user of NGX on `device` gone. Once none is left anywhere, NGX
/// shuts down, and the devices are returned to be released outside the lock:
/// a final D3D device release can wait for display removal.
fn release(
    devices: &mut BTreeMap<usize, Runtime>,
    device: &ID3D11Device,
) -> BTreeMap<usize, Runtime> {
    if let Some(runtime) = devices.get_mut(&(device.as_raw() as usize)) {
        runtime.users = runtime.users.saturating_sub(1);
    }
    if devices.values().all(|d| d.users == 0) {
        for runtime in devices.values() {
            // SAFETY: NGX was initialized on this live device, and no feature on it remains.
            unsafe {
                NVSDK_NGX_D3D11_Shutdown1(runtime.device.as_raw());
            }
        }
        std::mem::take(devices)
    } else {
        BTreeMap::new()
    }
}
/// Whether NGX reports `feature` available, from fresh capability parameters.
/// The caller holds the lock and has acquired NGX on the device.
unsafe fn feature_available(feature: Feature) -> Result<*mut c_void, u32> {
    unsafe {
        let mut params = ptr::null_mut();
        check(NVSDK_NGX_D3D11_GetCapabilityParameters(&mut params))?;
        if params.is_null() {
            return Err(ERROR_NULL_PARAMETERS);
        }
        let mut available = 0;
        let result = check(NVSDK_NGX_Parameter_GetI(
            params,
            feature.available().as_ptr(),
            &mut available,
        ));
        if result.is_err() || available == 0 {
            NVSDK_NGX_D3D11_DestroyParameters(params);
            return Err(result.err().unwrap_or(ERROR_UNAVAILABLE));
        }
        Ok(params)
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
            // On failure, Drop releases what was made.
            let mut devices = DEVICES.lock().unwrap();
            acquire(&mut devices, &state.device)?;
            state.acquired = true;
            state.params = feature_available(feature)?;
            check(NVSDK_NGX_D3D11_CreateFeature(
                state.context.as_raw(),
                feature.id(),
                state.params,
                &mut state.handle,
            ))?;
            if state.handle.is_null() {
                return Err(ERROR_FEATURE);
            }
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
                let mut desc = D3D11_TEXTURE2D_DESC {
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
                if self
                    .device
                    .CreateTexture2D(&desc, None, Some(&mut self.output))
                    .is_err()
                {
                    // An 8-bit BGRA texture can lack typed UAV support; NGX
                    // then writes it as a render target.
                    desc.BindFlags =
                        (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32;
                    self.output = None;
                    self.device
                        .CreateTexture2D(&desc, None, Some(&mut self.output))
                        .map_err(|e| e.code().0 as u32)?;
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
                    (c"TrueHDR.MaxLuminance", peak.clamp(400, 1000)),
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
                BTreeMap::new()
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
    2
}
/// Whether `feature` (0 TrueHDR, 1 VSR) is available on `device`: 1 if so, 0
/// with `error` set otherwise. NGX is initialized for the check and shut down
/// after it unless a converter still uses it.
/// # Safety
/// `device` must be null or a valid ID3D11Device interface; a non-null `error`
/// must point to a writable u32.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn butterpollo_ngx_available(
    device: *mut c_void,
    feature: u32,
    error: *mut u32,
) -> u32 {
    report(error, 0);
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
    let (result, releases) = {
        let mut devices = DEVICES.lock().unwrap();
        match acquire(&mut devices, device) {
            Err(code) => (Err(code), BTreeMap::new()),
            Ok(()) => {
                // SAFETY: the lock is held and NGX is acquired on `device`.
                let result = unsafe { feature_available(feature) }.map(|params| {
                    // SAFETY: `params` came from GetCapabilityParameters.
                    unsafe { NVSDK_NGX_D3D11_DestroyParameters(params) };
                });
                (result, release(&mut devices, device))
            }
        }
    };
    drop(releases);
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
/// must point to a writable u32. Destroy the returned state exactly once.
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
/// writable. Calls on one state must be serialized and use its owning thread.
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
/// owning thread.
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
