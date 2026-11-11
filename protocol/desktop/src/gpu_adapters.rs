//! The GPUs Direct3D 11 offers on Windows. SDL's Direct3D 11 renderer draws on the first adapter,
//! which a laptop with a dedicated GPU beside its integrated one sets by its graphics preference,
//! and D3D11VA decodes on the renderer's device. An integrated GPU without a 4:4:4 decoder then
//! refuses YUV 4:4:4 that the dedicated one decodes: this names both, so the player can choose.

use std::ffi::c_void;

use windows::core::{Interface, GUID};

use crate::strings::trf;
use windows::Win32::Foundation::{HMODULE, LUID};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11VideoDevice, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_AYUV, DXGI_FORMAT_Y410};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter1, IDXGIDevice, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};

/// DXVA's HEVC Range Extension profiles FFmpeg decodes HEVC 4:4:4 with (`ff_DXVA2_ModeHEVC_VLD_Main_444`
/// and `_Main10_444` in its dxva2.c; FFmpeg 8.1's avcodec carries both).
const HEVC_MAIN_444: GUID = GUID::from_values(0x4008_018f, 0xf537, 0x4b36, [0x98, 0xcf, 0x61, 0xaf, 0x8a, 0x2c, 0x1a, 0x33]);
const HEVC_MAIN10_444: GUID = GUID::from_values(0x0dab_effa, 0x4458, 0x4602, [0xbc, 0x03, 0x07, 0x95, 0x65, 0x9d, 0x61, 0x7c]);

/// The profile and the surface format a decoder of HEVC 4:4:4 (`ten_bit`: 10-bit) decodes into.
fn hevc444_profile(ten_bit: bool) -> (GUID, DXGI_FORMAT) {
    if ten_bit { (HEVC_MAIN10_444, DXGI_FORMAT_Y410) } else { (HEVC_MAIN_444, DXGI_FORMAT_AYUV) }
}

fn name_of(description: &[u16]) -> String {
    let end = description.iter().position(|&c| c == 0).unwrap_or(description.len());
    String::from_utf16_lossy(&description[..end]).trim().to_owned()
}

/// The renderer's GPU: its name and LUID. `device` is the renderer's `ID3D11Device`.
fn renderer_adapter(device: *mut c_void) -> Option<(String, LUID)> {
    // SAFETY: A live device pointer from the renderer, only borrowed here.
    let device = unsafe { ID3D11Device::from_raw_borrowed(&device) }?;
    // SAFETY: Plain DXGI queries on the borrowed device.
    unsafe {
        let desc = device.cast::<IDXGIDevice>().ok()?.GetAdapter().ok()?.GetDesc().ok()?;
        Some((name_of(&desc.Description), desc.AdapterLuid))
    }
}

/// The name of the GPU the renderer draws and D3D11VA decodes on, for the log.
pub fn renderer_gpu(device: *mut c_void) -> Option<String> {
    renderer_adapter(device).map(|(name, _)| name)
}

/// Whether `adapter`'s video decoder has `profile` and decodes it into `format`.
unsafe fn decodes(adapter: &IDXGIAdapter1, (profile, format): (GUID, DXGI_FORMAT)) -> bool {
    let mut device = None;
    if D3D11CreateDevice(adapter, D3D_DRIVER_TYPE_UNKNOWN, HMODULE::default(), D3D11_CREATE_DEVICE_VIDEO_SUPPORT, None,
        D3D11_SDK_VERSION, Some(&mut device), None, None).is_err() {
        return false;
    }
    let Some(video) = device.and_then(|device| device.cast::<ID3D11VideoDevice>().ok()) else { return false };
    (0..video.GetVideoDecoderProfileCount()).any(|index| {
        video.GetVideoDecoderProfile(index).is_ok_and(|found| found == profile)
            && video.CheckVideoDecoderFormat(&profile, format).is_ok_and(|ok| ok.as_bool())
    })
}

/// The GPUs other than the renderer's, by name, that decode HEVC 4:4:4 (`ten_bit`: Main10 4:4:4),
/// with the renderer's: None when the renderer's device is unknown.
pub fn others_decoding_hevc444(device: *mut c_void, ten_bit: bool) -> Option<(String, Vec<String>)> {
    let (renderer, luid) = renderer_adapter(device)?;
    let mut others = Vec::new();
    // SAFETY: Plain DXGI and D3D11 calls; every device made here is its own and dropped here.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        for index in 0.. {
            let Ok(adapter) = factory.EnumAdapters1(index) else { break };
            let Ok(desc) = adapter.GetDesc1() else { continue };
            let same = desc.AdapterLuid.LowPart == luid.LowPart && desc.AdapterLuid.HighPart == luid.HighPart;
            if same || desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            if decodes(&adapter, hevc444_profile(ten_bit)) {
                others.push(name_of(&desc.Description));
            }
        }
    }
    Some((renderer, others))
}

/// What to tell the player when the renderer's GPU (`renderer`) can't decode 4:4:4 and `other`
/// can: Windows' per-app graphics preference moves Nightjar to it from its next start.
pub fn hint(renderer: &str, other: &str) -> String {
    trf("Nightjar draws on {}, which can’t; {} can: set Nightjar to High performance in Windows Settings › System › Display › Graphics, then restart Nightjar", &[&renderer, &other])
}

#[cfg(test)]
mod tests {
    /// The profiles are the ones FFmpeg's D3D11VA asks for, as its dxva2.c writes them.
    #[test]
    fn hevc_444_profiles_are_ffmpegs() {
        assert_eq!(format!("{:?}", super::HEVC_MAIN_444).to_ascii_lowercase(), "4008018f-f537-4b36-98cf-61af8a2c1a33");
        assert_eq!(format!("{:?}", super::HEVC_MAIN10_444).to_ascii_lowercase(), "0dabeffa-4458-4602-bc03-0795659d617c");
    }

    /// The check runs on this machine's GPUs without failing, and never lists the renderer's own.
    #[test]
    fn other_gpus_are_checked_without_the_renderers() {
        use windows::core::Interface;
        use windows::Win32::Foundation::HMODULE;
        use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
        use windows::Win32::Graphics::Direct3D11::*;
        let mut device = None;
        // SAFETY: A device of the test's own.
        let made = unsafe {
            D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_VIDEO_SUPPORT, None,
                D3D11_SDK_VERSION, Some(&mut device), None, None)
        };
        let Some(device) = device.filter(|_| made.is_ok()) else {
            eprintln!("no hardware Direct3D 11 device here");
            return;
        };
        let raw = device.as_raw();
        let name = super::renderer_gpu(raw).expect("the device's GPU");
        assert!(!name.is_empty());
        for ten_bit in [false, true] {
            let (renderer, others) = super::others_decoding_hevc444(raw, ten_bit).expect("the renderer's GPU");
            assert_eq!(renderer, name);
            eprintln!("renderer {renderer}; others decoding HEVC 4:4:4 (10-bit {ten_bit}): {others:?}");
        }
    }

    #[test]
    fn the_hint_names_both_gpus() {
        let hint = super::hint("AMD Radeon 780M Graphics", "NVIDIA GeForce RTX 5060 Laptop GPU");
        assert!(hint.contains("AMD Radeon 780M Graphics") && hint.contains("NVIDIA GeForce RTX 5060 Laptop GPU"), "{hint}");
    }
}
