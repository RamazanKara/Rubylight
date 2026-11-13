//! AMD Direct Capture (`capture = amd`): AMF's display capture component
//! (`AMFDisplayCapture`) reads the Radeon driver's own presented surface, the
//! one AMD Fluid Motion Frames writes its generated frames into, where WGC and
//! Desktop Duplication take the desktop Windows composes.
//!
//! The capture mode, frame rate and Radeon-only setup follow Foundation
//! Sunshine's AMD Direct Capture (src/platform/windows/display_amd.cpp, its
//! AFMF capture fixes in commits 02d41dc "fix AFMF capture with AMD capture"
//! (#151, 2025-07-25) and 804ad30 (#175, 2025-08-04), released in
//! v2025.801.111024 and v2025.810), Copyright (c) Foundation Sunshine
//! authors, GPL-3.0: `WAIT_FOR_PRESENT` instead of `GET_CURRENT_SURFACE`,
//! the stream's frame rate instead of 0, and a duplicated output surface.
use super::{Device, GpuImage, GpuPool, Image, read_texture};
use crate::{amf::check, amf_abi::*, text::static_wide};
use anyhow::{Context, Result, bail};
use std::{ptr, time::Instant};
use windows::{Win32::Graphics::Direct3D11::*, core::Interface};

/// AMD Direct Capture needs AMF 1.4.30 or newer (Foundation Sunshine's floor).
const MINIMUM_VERSION: u64 = (1 << 48) | (4 << 32) | (30 << 16);
/// AMD's PCI vendor ID.
const AMD: u32 = 0x1002;

/// When the driver presented a captured picture, from the surface's properties.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Presented {
    /// `FrameIndex`: pictures the display presented since capture started.
    pub index: i64,
    /// `FlipTimesamp` (sic): the driver's flip time, in AMF's 100 ns units.
    pub flip: i64,
}

pub struct Session {
    pub gpu: Device,
    _library: libloading::Library,
    context: *mut AMFContext,
    component: *mut AMFComponent,
    /// The picture last copied, kept until the next so AMF does not reuse it
    /// while the copy may still read it.
    held: *mut AMFSurface,
    pub(crate) owned: GpuPool,
    staging: Option<ID3D11Texture2D>,
    /// The last picture's present index and flip time.
    pub last: Option<Presented>,
    /// Presents the capture skipped (`FrameIndex` gaps).
    pub skipped: u64,
}
// SAFETY: The AMF objects are used only through `&mut self` on the capture thread; the D3D11
// device is multithread-protected (`Device::create`).
unsafe impl Send for Session {}

fn int(value: i64) -> AMFVariantStruct {
    crate::amf::int(value)
}
fn set(component: *mut AMFComponent, name: &'static str, value: AMFVariantStruct) -> Result<()> {
    // SAFETY: `component` is live, `name` NUL-terminated, and AMF copies the variant.
    unsafe {
        check(((*(*component).pVtbl).SetProperty.unwrap())(
            component,
            static_wide(name).as_ptr(),
            value,
        ))
        .with_context(|| format!("AMD Direct Capture {name}"))
    }
}
/// An int64 property of `surface`, if it has one.
///
/// # Safety
///
/// `surface` is a live AMF surface.
unsafe fn surface_int(surface: *mut AMFSurface, name: &'static str) -> Option<i64> {
    let mut value = int(0);
    // SAFETY: The caller passes a live surface; `value` is initialized and only an INT64
    // result is read.
    unsafe {
        let result = ((*(*surface).pVtbl).GetProperty.unwrap())(
            surface,
            static_wide(name).as_ptr(),
            &mut value,
        );
        // The union is read only when GetProperty succeeded with an INT64 value.
        if result == AMF_RESULT_AMF_OK && value.type_ == AMF_VARIANT_TYPE_AMF_VARIANT_INT64 {
            Some(value.__bindgen_anon_1.int64Value)
        } else {
            None
        }
    }
}

impl Session {
    /// Capture display `name` on its own GPU, at most at its refresh rate.
    pub fn new(name: &str) -> Result<Self> {
        let gpu = Device::for_display(name, "", "")?;
        // Foundation Sunshine sets the stream's rate; a display's refresh
        // bounds every rate it can present, generated frames included.
        let fps_millihz = match super::current_refresh_hz(&gpu.display.display_name) {
            0 => 60_000,
            hz => hz.saturating_mul(1000),
        };
        if gpu.vendor != AMD {
            bail!(
                "AMD Direct Capture needs the display on a Radeon; {} is on another vendor's GPU",
                gpu.display.friendly_name
            );
        }
        // SAFETY: The runtime exports AMFQueryVersion and AMFInit with the SDK's C ABI; the
        // factory, context and component are owned interfaces released in Drop (or below on
        // error), and the library outlives them.
        unsafe {
            let library =
                libloading::Library::new("amfrt64.dll").context("AMD AMF runtime unavailable")?;
            let query: libloading::Symbol<unsafe extern "C" fn(*mut u64) -> AMF_RESULT> =
                library.get(b"AMFQueryVersion\0")?;
            let mut runtime = 0u64;
            check(query(&mut runtime)).context("AMFQueryVersion")?;
            if runtime < MINIMUM_VERSION {
                bail!(
                    "AMD Direct Capture needs AMF 1.4.30 or newer; the driver has {}. Update the Radeon driver",
                    butterpollo_core::codec_support::amf_version_text(runtime)
                );
            }
            let init: libloading::Symbol<
                unsafe extern "C" fn(u64, *mut *mut AMFFactory) -> AMF_RESULT,
            > = library.get(b"AMFInit\0")?;
            let mut factory = ptr::null_mut();
            // The runtime's own version, so an older driver accepts it.
            check(init(runtime.min(header_version()), &mut factory)).context("AMFInit")?;
            if factory.is_null() {
                bail!("AMF returned no factory");
            }
            let mut context = ptr::null_mut();
            check(((*(*factory).pVtbl).CreateContext.unwrap())(
                factory,
                &mut context,
            ))?;
            let mut session = Self {
                gpu,
                _library: library,
                context,
                component: ptr::null_mut(),
                held: ptr::null_mut(),
                owned: GpuPool::default(),
                staging: None,
                last: None,
                skipped: 0,
            };
            check(((*(*context).pVtbl).InitDX11.unwrap())(
                context,
                session.gpu.device.as_raw(),
                AMF_DX_VERSION_AMF_DX11_1,
            ))
            .context("AMF InitDX11")?;
            check(((*(*factory).pVtbl).CreateComponent.unwrap())(
                factory,
                context,
                static_wide("AMFDisplayCapture").as_ptr(),
                &mut session.component,
            ))
            .context("AMF CreateComponent AMFDisplayCapture")?;
            if session.component.is_null() {
                bail!("AMF returned no display capture");
            }
            let c = session.component;
            set(
                c,
                "MonitorIndex",
                int(i64::from(session.gpu.display.output_index)),
            )?;
            set(
                c,
                "FrameRate",
                AMFVariantStruct {
                    type_: AMF_VARIANT_TYPE_AMF_VARIANT_RATE,
                    __bindgen_anon_1: AMFVariantStruct__bindgen_ty_1 {
                        rateValue: AMFRate {
                            num: fps_millihz.max(1000),
                            den: 1000,
                        },
                    },
                },
            )?;
            set(
                c,
                "CaptureMode",
                int(AMF_DISPLAYCAPTURE_MODE_ENUM_AMF_DISPLAYCAPTURE_MODE_WAIT_FOR_PRESENT as i64),
            )?;
            set(c, "DuplicateOutput", crate::amf::gpu::boolean(true))?;
            // WGC and the display driver show the pointer; so does this.
            set(c, "EmbedCursor", crate::amf::gpu::boolean(true))?;
            check(((*(*c).pVtbl).Init.unwrap())(
                c,
                AMF_SURFACE_FORMAT_AMF_SURFACE_UNKNOWN,
                0,
                0,
            ))
            .context("AMD Direct Capture Init")?;
            tracing::info!(
                display = %session.gpu.display.friendly_name,
                output = session.gpu.display.output_index,
                runtime = butterpollo_core::codec_support::amf_version_text(runtime),
                fps_millihz,
                "AMD Direct Capture started (waits for each present)"
            );
            Ok(session)
        }
    }
    fn release_held(&mut self) {
        if !self.held.is_null() {
            // SAFETY: `held` is the one reference this session took in `next_gpu`.
            unsafe {
                ((*(*self.held).pVtbl).Release.unwrap())(self.held);
            }
            self.held = ptr::null_mut();
        }
    }
    /// The newest presented picture, copied into a texture the stream owns,
    /// or `None` when nothing was presented since the last call.
    pub fn next_gpu(&mut self) -> Result<Option<GpuImage>> {
        if !self.owned.has_capacity() {
            return Ok(None);
        }
        // SAFETY: `component` is live; QueryOutput hands an owned reference, queried for its
        // surface interface and released once; the surface's plane texture is borrowed while
        // the surface is held.
        unsafe {
            let mut data = ptr::null_mut();
            let result =
                ((*(*self.component).pVtbl).QueryOutput.unwrap())(self.component, &mut data);
            if data.is_null() {
                if matches!(result, AMF_RESULT_AMF_OK | AMF_RESULT_AMF_REPEAT) {
                    return Ok(None);
                }
                check(result).context("AMD Direct Capture QueryOutput")?;
                return Ok(None);
            }
            let mut queried = ptr::null_mut();
            let result = ((*(*data).pVtbl).QueryInterface.unwrap())(
                data,
                &crate::amf::focus::SURFACE_IID,
                &mut queried,
            );
            ((*(*data).pVtbl).Release.unwrap())(data);
            check(result).context("AMD Direct Capture output is not a surface")?;
            if queried.is_null() {
                bail!("AMD Direct Capture returned no surface");
            }
            let surface: *mut AMFSurface = queried.cast();
            let presented = Presented {
                index: surface_int(surface, "FrameIndex").unwrap_or(0),
                flip: surface_int(surface, "FlipTimesamp").unwrap_or(0),
            };
            if let Some(last) = self.last
                && presented.index > last.index + 1
            {
                self.skipped += (presented.index - last.index - 1) as u64;
            }
            self.last = Some(presented);
            // The previous picture's copy was queued before this one.
            self.release_held();
            self.held = surface;
            let plane = ((*(*surface).pVtbl).GetPlaneAt.unwrap())(surface, 0);
            if plane.is_null() {
                bail!("AMD Direct Capture surface has no plane");
            }
            let raw = ((*(*plane).pVtbl).GetNative.unwrap())(plane);
            let texture = ID3D11Texture2D::from_raw_borrowed(&raw)
                .context("AMD Direct Capture surface has no D3D11 texture")?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            if desc.ArraySize != 1 {
                bail!(
                    "AMD Direct Capture returned a texture array ({} slices), which the stream cannot read",
                    desc.ArraySize
                );
            }
            let image = self.owned.copy(&self.gpu, texture)?;
            Ok(image.map(|mut image| {
                image.captured = Instant::now();
                image
            }))
        }
    }
    /// The newest presented picture in CPU memory.
    pub fn next_frame(&mut self) -> Result<Option<Image>> {
        let Some(image) = self.next_gpu()? else {
            return Ok(None);
        };
        let mut read = read_texture(&self.gpu, &image.texture, &mut self.staging)?;
        read.captured = image.captured;
        Ok(Some(read))
    }
}
/// This build's AMF header version.
fn header_version() -> u64 {
    ((AMF_VERSION_MAJOR as u64) << 48)
        | ((AMF_VERSION_MINOR as u64) << 32)
        | ((AMF_VERSION_RELEASE as u64) << 16)
        | AMF_VERSION_BUILD_NUM as u64
}
impl Drop for Session {
    fn drop(&mut self) {
        self.release_held();
        if let Some(last) = self.last {
            tracing::info!(
                presents = last.index,
                missed = self.skipped,
                "AMD Direct Capture stopped"
            );
        }
        // SAFETY: Self owns the component and context references; the component terminates
        // before the context, and the library outlives both.
        unsafe {
            if !self.component.is_null() {
                ((*(*self.component).pVtbl).Terminate.unwrap())(self.component);
                ((*(*self.component).pVtbl).Release.unwrap())(self.component);
            }
            if !self.context.is_null() {
                ((*(*self.context).pVtbl).Terminate.unwrap())(self.context);
                ((*(*self.context).pVtbl).Release.unwrap())(self.context);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_minimum_runtime_is_amf_1_4_30() {
        assert_eq!(
            butterpollo_core::codec_support::amf_version_text(MINIMUM_VERSION),
            "1.4.30.0"
        );
        assert!(header_version() > MINIMUM_VERSION);
    }

    #[test]
    fn the_capture_mode_waits_for_each_present() {
        assert_eq!(
            AMF_DISPLAYCAPTURE_MODE_ENUM_AMF_DISPLAYCAPTURE_MODE_WAIT_FOR_PRESENT,
            1
        );
    }
}
