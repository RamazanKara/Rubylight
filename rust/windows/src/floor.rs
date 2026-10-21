//! What this PC offers against a release's floor ([`butterpollo_core::floor`]): the Windows
//! build, an RDNA 2 or newer AMD adapter on which AMF creates the HEVC encoder, and
//! Rubylight's running display driver. Setup runs it through `butterpollo.exe --floor-check` before it changes anything;
//! the updater asks it before offering a release.
//!
//! The GPU is never judged by name or device ID. Each AMD adapter must create a D3D12 device
//! at feature level 12_2 (DirectX 12 Ultimate, which Radeon gained with RDNA 2; RDNA 1, Vega
//! and Polaris stop at 12_1), then an AMF context on it and an HEVC encoder initialized at
//! 1280x720.

use anyhow::{Context, Result, bail};
use butterpollo_core::floor::{Adapter, Pc};
use std::ptr;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
};

use crate::amf::check;
use crate::amf_abi::*;
use crate::text::to_wide;

/// AMD's PCI vendor ID.
const AMD: u32 = 0x1002;

/// The Windows build (`CurrentBuildNumber`), or `None` where the registry does not say.
pub fn windows_build() -> Option<u32> {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let mut text = [0u16; 32];
    let mut size = (text.len() * 2) as u32;
    // SAFETY: the key and value names are terminated literals, and `size` is the byte length
    // of the live UTF-16 buffer RegGetValueW writes into.
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            windows::core::w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
            windows::core::w!("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            None,
            Some(text.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .ok()
        .ok()?;
    }
    let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
    String::from_utf16_lossy(&text[..end]).trim().parse().ok()
}

/// Every hardware adapter, and for AMD's whether it passed the GPU check.
pub fn adapters() -> Result<Vec<Adapter>> {
    // SAFETY: a plain factory creation; the adapters it enumerates are owned interfaces.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let mut found = Vec::new();
    for index in 0.. {
        // SAFETY: enumeration ends with an error past the last adapter.
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        // SAFETY: a plain query of the live adapter.
        let desc = unsafe { adapter.GetDesc1()? };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let end = desc.Description.iter().position(|&c| c == 0).unwrap_or(0);
        let name = String::from_utf16_lossy(&desc.Description[..end]);
        let checked = (desc.VendorId == AMD).then(|| {
            ultimate(&adapter)
                .and_then(|()| hevc_encoder(desc.AdapterLuid))
                .map(|()| String::new())
                .unwrap_or_else(|error| format!("{error:#}"))
        });
        found.push(Adapter {
            name,
            vendor: desc.VendorId,
            hevc_error: checked.clone().filter(|e| !e.is_empty()),
            hevc: checked.is_some_and(|e| e.is_empty()),
        });
    }
    Ok(found)
}

/// Whether Rubylight's display driver can carry a stream: it runs and has the fence ring.
/// An older driver that merely answers does not count; it needs a restart or a repair.
pub fn display_driver() -> bool {
    driver_present(crate::display::driver_caps())
}

/// The floor's reading of the driver as [`crate::display::driver_caps`] found it.
fn driver_present(opened: Result<u32>) -> bool {
    crate::display::driver_can_stream(opened).is_ok()
}

/// This PC as the floor sees it.
pub fn pc() -> Result<Pc> {
    Ok(Pc {
        windows_build: windows_build(),
        adapters: adapters()?,
        display_driver: display_driver(),
    })
}

/// Whether `adapter` creates a D3D12 device at feature level 12_2.
fn ultimate(adapter: &windows::Win32::Graphics::Dxgi::IDXGIAdapter1) -> Result<()> {
    use windows::Win32::Graphics::{Direct3D::D3D_FEATURE_LEVEL_12_2, Direct3D12::*};
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: `adapter` is live and `device` a live local; the device is released at once.
    unsafe { D3D12CreateDevice(adapter, D3D_FEATURE_LEVEL_12_2, &mut device) }
        .context("D3D12 feature level 12_2 (RDNA 2 or newer)")?;
    Ok(())
}

/// Create and initialize AMF's HEVC encoder on the adapter `luid`, then release it.
fn hevc_encoder(luid: windows::Win32::Foundation::LUID) -> Result<()> {
    let compute = crate::compute::Compute::new(luid).context("D3D12 device")?;
    // SAFETY: amfrt64.dll exports AMFInit with the SDK's C ABI. Every interface a successful
    // call returns is live until released here, in reverse order; names are terminated
    // temporaries AMF copies synchronously.
    unsafe {
        let library = crate::telemetry::system_library("amfrt64.dll")
            .context("AMD AMF runtime unavailable")?;
        let init: libloading::Symbol<
            unsafe extern "C" fn(u64, *mut *mut AMFFactory) -> AMF_RESULT,
        > = library.get(b"AMFInit\0")?;
        let version = ((AMF_VERSION_MAJOR as u64) << 48)
            | ((AMF_VERSION_MINOR as u64) << 32)
            | ((AMF_VERSION_RELEASE as u64) << 16)
            | AMF_VERSION_BUILD_NUM as u64;
        let mut factory = ptr::null_mut();
        check(init(version, &mut factory)).context("AMFInit")?;
        if factory.is_null() {
            bail!("AMF returned no factory");
        }
        let mut context = ptr::null_mut();
        check(((*(*factory).pVtbl).CreateContext.unwrap())(
            factory,
            &mut context,
        ))
        .context("AMF CreateContext")?;
        let mut component = ptr::null_mut();
        let result = (|| -> Result<()> {
            let d3d12 = crate::amf::gpu::D3d12Context::new(context)?;
            d3d12.init(&compute.device)?;
            check(((*(*factory).pVtbl).CreateComponent.unwrap())(
                factory,
                context,
                to_wide("AMFVideoEncoderHW_HEVC").as_ptr(),
                &mut component,
            ))
            .context("AMF CreateComponent AMFVideoEncoderHW_HEVC")?;
            check(((*(*component).pVtbl).Init.unwrap())(
                component,
                AMF_SURFACE_FORMAT_AMF_SURFACE_NV12,
                1280,
                720,
            ))
            .context("AMF HEVC encoder Init")?;
            ((*(*component).pVtbl).Terminate.unwrap())(component);
            drop(d3d12);
            Ok(())
        })();
        if !component.is_null() {
            ((*(*component).pVtbl).Release.unwrap())(component);
        }
        ((*(*context).pVtbl).Terminate.unwrap())(context);
        ((*(*context).pVtbl).Release.unwrap())(context);
        result
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_floor_counts_a_driver_only_when_it_can_carry_a_stream() {
        use rubylight_vdisplay_proto::control::{CAP_FENCE_RING, CAP_HDR};
        // A driver whose control device opens but lacks the fence ring is not present.
        assert!(!super::driver_present(Ok(CAP_HDR)));
        assert!(!super::driver_present(Ok(0)));
        assert!(!super::driver_present(Err(anyhow::anyhow!(
            "no control device"
        ))));
        assert!(super::driver_present(Ok(CAP_FENCE_RING)));
    }
}
