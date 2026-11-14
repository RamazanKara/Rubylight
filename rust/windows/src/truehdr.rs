//! RTX HDR (NVIDIA TrueHDR) through the NVIDIA component's NGX adapter: an SDR
//! capture turned into an FP16 HDR picture before encoding. Creation,
//! conversion and teardown stay on one thread.
use crate::capture::{Device, GpuImage, GpuPool, Pixel};
use crate::nvidia_enhance::describe;
use anyhow::{Context, Result, bail};
use std::{ffi::c_void, ptr};
use windows::{Win32::Graphics::Direct3D11::*, core::Interface};
type Create = unsafe extern "C" fn(*mut c_void, *mut u32) -> *mut c_void;
type Convert =
    unsafe extern "C" fn(*mut c_void, *mut c_void, u32, u32, u32, u32, *mut u32) -> *mut c_void;
type Destroy = unsafe extern "C" fn(*mut c_void);
pub struct Filter {
    state: *mut c_void,
    convert: Convert,
    destroy: Destroy,
    device: Device,
    parameters: [u32; 4],
    pool: GpuPool,
    _dll: libloading::Library,
}
impl Filter {
    /// NGX's contrast, saturation, middle gray and peak
    /// (`butterpollo_core::nvidia_enhance::rtx_parameters`).
    pub fn set_parameters(&mut self, parameters: [u32; 4]) {
        self.parameters = parameters;
    }
    /// The parameters the last conversion used.
    pub fn parameters(&self) -> [u32; 4] {
        self.parameters
    }
    /// A converter on the capture's device.
    pub fn new_gpu(image: &GpuImage, parameters: [u32; 4]) -> Result<Self> {
        Self::new_device(image.gpu.clone(), parameters)
    }
    fn new_device(device: Device, parameters: [u32; 4]) -> Result<Self> {
        if device.vendor != 0x10de {
            bail!("RTX HDR needs an NVIDIA GPU");
        }
        let dll = crate::nvidia_enhance::load()?;
        // SAFETY: The adapter's ABI 3 (checked by `load`) exports these symbols with these types,
        // and Filter keeps `dll` and `device` while the pointers are used.
        unsafe {
            let create = *dll.get::<Create>(b"butterpollo_truehdr_create\0")?;
            let convert = *dll.get::<Convert>(b"butterpollo_truehdr_convert\0")?;
            let destroy = *dll.get::<Destroy>(b"butterpollo_truehdr_destroy\0")?;
            let mut error = 0;
            let state = create(device.device.as_raw(), &mut error);
            if state.is_null() {
                bail!("RTX HDR could not start: {}", describe(error));
            }
            Ok(Self {
                state,
                convert,
                destroy,
                device,
                parameters,
                pool: GpuPool::default(),
                _dll: dll,
            })
        }
    }
    /// The HDR picture for an SDR capture. NGX's output is copied into a
    /// bounded pool before another conversion can overwrite it.
    pub fn apply_gpu(&mut self, image: &GpuImage) -> Result<GpuImage> {
        if image.pixel != Pixel::Bgra8 || image.gpu.device.as_raw() != self.device.device.as_raw() {
            bail!("RTX HDR needs an SDR picture on the capture's GPU");
        }
        crate::rtx_video::ready_for_d3d11(image)?;
        // SAFETY: `state` is the live NGX state, created on this device. The adapter takes its own
        // lock and then the device's multithread lock (ABI 3), so this thread holds neither while
        // it converts; the returned texture is only borrowed while it is copied into the pool,
        // under the device's lock.
        unsafe {
            let [contrast, saturation, middle, peak] = self.parameters;
            let mut error = 0;
            let output = (self.convert)(
                self.state,
                image.texture.as_raw(),
                contrast,
                saturation,
                middle,
                peak,
                &mut error,
            );
            let texture = ID3D11Texture2D::from_raw_borrowed(&output)
                .with_context(|| format!("RTX HDR conversion failed: {}", describe(error)))?;
            let lock: ID3D11Multithread = self.device.context.cast()?;
            lock.Enter();
            let result = (|| {
                let mut converted = self
                    .pool
                    .copy(&self.device, texture)?
                    .context("the RTX HDR pictures are all still being encoded")?;
                converted.inherit(image);
                Ok(converted)
            })();
            // The HDR picture is read on the compute queue through a shared handle: flush the
            // D3D11 commands that wrote it before it is handed on.
            self.device.context.Flush();
            lock.Leave();
            result
        }
    }
}
impl Drop for Filter {
    fn drop(&mut self) {
        // SAFETY: `state` came from create and is destroyed once, here; `_dll` is dropped after
        // this.
        unsafe {
            (self.destroy)(self.state);
        }
        self.state = ptr::null_mut();
    }
}
