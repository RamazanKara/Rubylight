//! AMD's video super resolution: the AMF runtime's HQ Scaler, loaded at run time and called
//! through AMF's C ABI on the renderer's Direct3D 11 device.
//!
//! Ported from Moonlight V+ (qiin2333/moonlight-qt, `amfsession.cpp`): the same component, the
//! same properties and the same submit, query and flush per picture.

use std::ffi::c_void;
use std::ptr;

use windows::core::{Interface, GUID};
use windows::Win32::Graphics::Direct3D11::*;

const OK: i32 = 0;
const MEMORY_DX11: i32 = 3;
const DX11_0: i32 = 110;
const SURFACE_NV12: i32 = 1;
const SURFACE_P010: i32 = 10;
const ACCEL_NOT_SUPPORTED: i32 = -1;
const ALGORITHM_VIDEOSR1_0: i64 = 2;
/// `AMF_MAKE_FULL_VERSION(1, 4, 30, 0)`: the runtime must be at least this new.
const VERSION: u64 = (1 << 48) | (4 << 32) | (30 << 16);
const SURFACE_IID: GUID = GUID::from_values(0x3075dbe3, 0x8718, 0x4cfa, [0x86, 0xfb, 0x21, 0x14, 0xc0, 0xa5, 0xa4, 0x51]);

/// Slots in AMF's C vtables (`AMFFactoryVtbl`, `AMFContextVtbl`, `AMFComponentVtbl`, ...).
mod slot {
    pub const RELEASE: usize = 1;
    pub const QUERY_INTERFACE: usize = 2;
    pub const SET_PROPERTY: usize = 3;
    pub const FACTORY_CREATE_CONTEXT: usize = 0;
    pub const FACTORY_CREATE_COMPONENT: usize = 1;
    pub const CONTEXT_TERMINATE: usize = 13;
    pub const CONTEXT_INIT_DX11: usize = 18;
    pub const CONTEXT_ALLOC_SURFACE: usize = 44;
    pub const CONTEXT_SURFACE_FROM_DX11: usize = 49;
    pub const COMPONENT_INIT: usize = 17;
    pub const COMPONENT_TERMINATE: usize = 19;
    pub const COMPONENT_FLUSH: usize = 21;
    pub const COMPONENT_SUBMIT_INPUT: usize = 22;
    pub const COMPONENT_QUERY_OUTPUT: usize = 23;
    pub const COMPONENT_GET_CAPS: usize = 26;
    pub const CAPS_ACCELERATION_TYPE: usize = 13;
    pub const SURFACE_PLANE_AT: usize = 25;
    pub const PLANE_NATIVE: usize = 4;
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// `AMFVariantStruct`: the type, then a 16-byte union aligned to 8.
#[repr(C)]
#[derive(Clone, Copy)]
struct Variant {
    kind: i32,
    value: [u64; 2],
}

impl Variant {
    fn boolean(value: bool) -> Self { Self { kind: 1, value: [u64::from(value), 0] } }
    fn int(value: i64) -> Self { Self { kind: 2, value: [value as u64, 0] } }
    fn double(value: f64) -> Self { Self { kind: 3, value: [value.to_bits(), 0] } }
    fn size(width: i32, height: i32) -> Self { Self { kind: 5, value: [u64::from(width as u32) | (u64::from(height as u32) << 32), 0] } }
    fn color(rgba: [u8; 4]) -> Self { Self { kind: 9, value: [u64::from(u32::from_le_bytes(rgba)), 0] } }
}

/// An AMF interface pointer, released when dropped.
struct Object(*mut c_void);

impl Object {
    /// The function in `index` of the object's vtable, as `F`.
    unsafe fn method<F: Copy>(&self, index: usize) -> F {
        debug_assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<*const c_void>());
        let table = *(self.0 as *const *const *const c_void);
        std::mem::transmute_copy(&*table.add(index))
    }

    unsafe fn call0(&self, index: usize) -> i32 {
        self.method::<unsafe extern "system" fn(*mut c_void) -> i32>(index)(self.0)
    }

    unsafe fn set(&self, name: &str, value: Variant) -> Result<(), String> {
        let name16 = wide(name);
        let result = self.method::<unsafe extern "system" fn(*mut c_void, *const u16, Variant) -> i32>(slot::SET_PROPERTY)(self.0, name16.as_ptr(), value);
        check(result, name)
    }
}

impl Drop for Object {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: An interface AMF handed out with a reference this object owns.
            unsafe { self.call0(slot::RELEASE) };
        }
    }
}

fn check(result: i32, call: &str) -> Result<(), String> {
    if result == OK { Ok(()) } else { Err(format!("AMF {call} failed ({result})")) }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Parameters {
    pub width: i32,
    pub height: i32,
    pub output_width: i32,
    pub output_height: i32,
    pub frame_rate: i32,
    pub ten_bit: bool,
    pub sharpness: f64,
}

/// V+'s sharpness: gentle (0.30) while upscaling SDR, full (2.00) for HDR or at the picture's size.
pub fn sharpness(upscaling: bool, hdr: bool) -> f64 {
    if hdr || !upscaling { 2.00 } else { 0.30 }
}

/// The HQ Scaler on one device. Fields drop in order: the component and the context before the
/// runtime library that holds their code.
pub struct HqScaler {
    scaler: Object,
    context: Object,
    parameters: Parameters,
    _library: libloading::Library,
}

// SAFETY: AMF objects are used only from the thread that owns the scaler; moving it between
// threads is fine (the runtime has no thread affinity).
unsafe impl Send for HqScaler {}

impl HqScaler {
    /// Loads the runtime and sets up the scaler for `parameters`, as V+'s `AmfSession::initialize`.
    /// `device` is borrowed; AMF takes its own reference.
    pub unsafe fn new(device: &ID3D11Device, parameters: Parameters) -> Result<Self, String> {
        let p = parameters;
        if p.width <= 0 || p.height <= 0 || p.output_width <= 0 || p.output_height <= 0 || p.frame_rate <= 0 {
            return Err(format!("AMF scaler parameters are not usable: {p:?}"));
        }
        let library = libloading::Library::new("amfrt64.dll")
            .or_else(|_| libloading::Library::new("amfrtlt64.dll"))
            .map_err(|e| format!("the AMF runtime (amfrt64.dll) is not installed: {e}"))?;
        let init = library.get::<unsafe extern "C" fn(u64, *mut *mut c_void) -> i32>(b"AMFInit\0")
            .map_err(|e| format!("the AMF runtime has no AMFInit: {e}"))?;
        let mut factory = ptr::null_mut();
        check(init(VERSION, &mut factory), "AMFInit")?;
        if factory.is_null() { return Err("AMFInit returned no factory".into()); }
        // The factory belongs to the runtime and is not reference counted.
        let factory = std::mem::ManuallyDrop::new(Object(factory));
        let mut context = ptr::null_mut();
        check(factory.method::<unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32>(slot::FACTORY_CREATE_CONTEXT)(factory.0, &mut context), "CreateContext")?;
        let context = Object(context);
        check(context.method::<unsafe extern "system" fn(*mut c_void, *mut c_void, i32) -> i32>(slot::CONTEXT_INIT_DX11)(context.0, device.as_raw(), DX11_0), "InitDX11")?;
        let name = wide("AMFHQScaler");
        let mut scaler = ptr::null_mut();
        let created = factory.method::<unsafe extern "system" fn(*mut c_void, *mut c_void, *const u16, *mut *mut c_void) -> i32>(slot::FACTORY_CREATE_COMPONENT)(factory.0, context.0, name.as_ptr(), &mut scaler);
        let result = Self { scaler: Object(scaler), context, parameters: p, _library: library };
        check(created, "CreateComponent(AMFHQScaler)")?;
        if result.scaler.0.is_null() { return Err("AMF has no HQ Scaler".into()); }
        let mut caps = ptr::null_mut();
        check(result.scaler.method::<unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32>(slot::COMPONENT_GET_CAPS)(result.scaler.0, &mut caps), "GetCaps")?;
        let caps = Object(caps);
        if caps.0.is_null() || caps.call0(slot::CAPS_ACCELERATION_TYPE) == ACCEL_NOT_SUPPORTED {
            return Err("the GPU does not accelerate AMF's HQ Scaler".into());
        }
        // V+ allocates a surface of the input's shape as part of its capability check.
        let mut surface = ptr::null_mut();
        check(result.context.method::<unsafe extern "system" fn(*mut c_void, i32, i32, i32, i32, *mut *mut c_void) -> i32>(slot::CONTEXT_ALLOC_SURFACE)(
            result.context.0, MEMORY_DX11, result.format(), p.width, p.height, &mut surface), "AllocSurface")?;
        drop(Object(surface));
        let s = &result.scaler;
        s.set("HQSOutputSize", Variant::size(p.output_width, p.output_height))?;
        s.set("HQScalerEngineType", Variant::int(i64::from(MEMORY_DX11)))?;
        s.set("HQScalerAlgorithm", Variant::int(ALGORITHM_VIDEOSR1_0))?;
        s.set("KeepAspectRatio", Variant::boolean(true))?;
        s.set("Fill", Variant::boolean(true))?;
        s.set("FillColor", Variant::color([0, 0, 0, 255]))?;
        s.set("HQScalerSharpness", Variant::double(p.sharpness))?;
        s.set("HQScalerFrameRate", Variant::int(i64::from(p.frame_rate)))?;
        result.init()?;
        Ok(result)
    }

    fn format(&self) -> i32 {
        if self.parameters.ten_bit { SURFACE_P010 } else { SURFACE_NV12 }
    }

    unsafe fn init(&self) -> Result<(), String> {
        let p = &self.parameters;
        check(self.scaler.method::<unsafe extern "system" fn(*mut c_void, i32, i32, i32) -> i32>(slot::COMPONENT_INIT)(self.scaler.0, self.format(), p.width, p.height), "Init")
    }

    /// Scales `input` (the input size, NV12 or P010) and copies the result into the top left of
    /// `output` (at least the output size, same format) on `context`.
    pub unsafe fn render(&mut self, input: &ID3D11Texture2D, output: &ID3D11Texture2D, context: &ID3D11DeviceContext) -> Result<(), String> {
        let mut surface = ptr::null_mut();
        check(self.context.method::<unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void, *mut c_void) -> i32>(slot::CONTEXT_SURFACE_FROM_DX11)(
            self.context.0, input.as_raw(), &mut surface, ptr::null_mut()), "CreateSurfaceFromDX11Native")?;
        let surface = Object(surface);
        if surface.0.is_null() { return Err("AMF made no surface of the picture".into()); }
        check(self.scaler.method::<unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32>(slot::COMPONENT_SUBMIT_INPUT)(self.scaler.0, surface.0), "SubmitInput")?;
        let mut data = ptr::null_mut();
        let queried = self.scaler.method::<unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32>(slot::COMPONENT_QUERY_OUTPUT)(self.scaler.0, &mut data);
        let data = Object(data);
        let flushed = self.scaler.call0(slot::COMPONENT_FLUSH);
        check(queried, "QueryOutput")?;
        check(flushed, "Flush")?;
        if data.0.is_null() { return Err("AMF's HQ Scaler returned no picture".into()); }
        let mut scaled = ptr::null_mut();
        check(data.method::<unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32>(slot::QUERY_INTERFACE)(data.0, &SURFACE_IID, &mut scaled), "QueryInterface(AMFSurface)")?;
        let scaled = Object(scaled);
        if scaled.0.is_null() { return Err("AMF's HQ Scaler returned no surface".into()); }
        // The plane belongs to the surface and is not reference counted.
        let plane = scaled.method::<unsafe extern "system" fn(*mut c_void, usize) -> *mut c_void>(slot::SURFACE_PLANE_AT)(scaled.0, 0);
        if plane.is_null() { return Err("AMF's scaled surface has no plane".into()); }
        let plane = std::mem::ManuallyDrop::new(Object(plane));
        let native = plane.method::<unsafe extern "system" fn(*mut c_void) -> *mut c_void>(slot::PLANE_NATIVE)(plane.0);
        let texture = ID3D11Texture2D::from_raw_borrowed(&native).ok_or("AMF's scaled surface is not a Direct3D 11 texture")?;
        // A box, not CopyResource: AMF may allocate its surfaces larger than the picture.
        let p = &self.parameters;
        context.CopySubresourceRegion(output, 0, 0, 0, 0, texture, 0,
            Some(&D3D11_BOX { left: 0, top: 0, front: 0, right: p.output_width as u32, bottom: p.output_height as u32, back: 1 }));
        Ok(())
    }
}

impl Drop for HqScaler {
    fn drop(&mut self) {
        // SAFETY: Live AMF objects; terminate before the references are released.
        unsafe {
            if !self.scaler.0.is_null() { self.scaler.call0(slot::COMPONENT_TERMINATE); }
            if !self.context.0.is_null() { self.context.call0(slot::CONTEXT_TERMINATE); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_have_amfs_layout() {
        assert_eq!(std::mem::size_of::<Variant>(), 24, "AMFVariantStruct is 24 bytes on x64");
        assert_eq!(std::mem::align_of::<Variant>(), 8);
        let size = Variant::size(3840, 2160);
        assert_eq!((size.kind, size.value[0] as u32, (size.value[0] >> 32) as u32), (5, 3840, 2160));
        // AMFColor is r, g, b, a bytes in that order.
        assert_eq!(Variant::color([1, 2, 3, 4]).value[0].to_le_bytes()[..4], [1, 2, 3, 4]);
        assert_eq!(Variant::boolean(true).value[0] & 0xFF, 1);
        assert_eq!(f64::from_bits(Variant::double(0.3).value[0]), 0.3);
    }

    #[test]
    fn sharpness_follows_v_plus() {
        assert_eq!(sharpness(true, false), 0.30);
        assert_eq!(sharpness(false, false), 2.00, "no upscale: sharpen only");
        assert_eq!(sharpness(true, true), 2.00, "HDR");
    }
}
