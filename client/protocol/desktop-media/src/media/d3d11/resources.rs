//! Shared NT-handle resource and texture-SRV preflight helpers for [`super::DecodeBridge`].

use windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE};
use windows::Win32::Graphics::Direct3D::D3D11_SRV_DIMENSION_TEXTURE2DARRAY;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::{
    DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE, IDXGIResource1,
};
use windows::core::{Interface, PCWSTR};

use super::super::Pictures;

/// RAII wrapper around an NT shared handle so early error returns never leak kernel handles.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: Owned handle created by `CreateSharedHandle` and closed once here.
        if !self.0.is_invalid() {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

pub(super) fn create_shared_fence_pair(
    decode_dev5: &ID3D11Device5,
    render_dev5: &ID3D11Device5,
    name: &str,
) -> Result<(ID3D11Fence, ID3D11Fence), String> {
    // SAFETY: Both `ID3D11Device5` instances are live; NT handle closes via `OwnedHandle`.
    unsafe {
        let mut dec_fence: Option<ID3D11Fence> = None;
        decode_dev5
            .CreateFence(0, D3D11_FENCE_FLAG_SHARED, &mut dec_fence)
            .map_err(|e| format!("creating shared D3D11 {name} fence: {e}"))?;
        let dec_fence = dec_fence.ok_or_else(|| format!("no D3D11 {name} fence returned"))?;
        let handle = OwnedHandle(
            dec_fence
                .CreateSharedHandle(None, GENERIC_ALL.0, PCWSTR::null())
                .map_err(|e| format!("exporting shared D3D11 {name} fence handle: {e}"))?,
        );
        let mut ren_fence: Option<ID3D11Fence> = None;
        render_dev5
            .OpenSharedFence(handle.0, &mut ren_fence)
            .map_err(|e| format!("opening shared D3D11 {name} fence on renderer device: {e}"))?;
        let ren_fence = ren_fence.ok_or_else(|| format!("no opened D3D11 {name} fence"))?;
        Ok((dec_fence, ren_fence))
    }
}

pub(super) fn create_shared_texture_pair(
    decode_device: &ID3D11Device,
    render_device1: &ID3D11Device1,
    format: DXGI_FORMAT,
    width: u32,
    height: u32,
) -> Result<(ID3D11Texture2D, ID3D11Texture2D), String> {
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
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0) as u32,
    };
    // SAFETY: Both devices are live; NT handle closes immediately via `OwnedHandle`.
    unsafe {
        let mut decode_tex = None;
        decode_device
            .CreateTexture2D(&desc, None, Some(&mut decode_tex))
            .map_err(|e| {
                format!(
                    "creating shared {width}x{height} format {} texture: {e}",
                    format.0
                )
            })?;
        let decode_tex = decode_tex.ok_or("CreateTexture2D returned no shared texture")?;
        let resource: IDXGIResource1 = decode_tex
            .cast()
            .map_err(|e| format!("querying IDXGIResource1 on shared texture: {e}"))?;
        let handle = OwnedHandle(
            resource
                .CreateSharedHandle(
                    None,
                    DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                    PCWSTR::null(),
                )
                .map_err(|e| format!("exporting shared texture NT handle: {e}"))?,
        );
        let render_tex: ID3D11Texture2D = render_device1
            .OpenSharedResource1(handle.0)
            .map_err(|e| format!("opening shared texture on renderer device: {e}"))?;
        Ok((decode_tex, render_tex))
    }
}

pub(super) fn preflight_formats(pictures: Pictures) -> &'static [DXGI_FORMAT] {
    match (pictures.chroma444, pictures.ten_bit) {
        (false, false) => &[DXGI_FORMAT_NV12],
        (false, true) => &[DXGI_FORMAT_P010],
        (true, false) => &[DXGI_FORMAT_AYUV],
        (true, true) => &[DXGI_FORMAT_Y410, DXGI_FORMAT_Y416],
    }
}

fn srv_formats_for(format: DXGI_FORMAT) -> &'static [DXGI_FORMAT] {
    match format {
        DXGI_FORMAT_NV12 => &[DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8_UNORM],
        DXGI_FORMAT_P010 => &[DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R16G16_UNORM],
        DXGI_FORMAT_AYUV => &[DXGI_FORMAT_R8G8B8A8_UNORM],
        DXGI_FORMAT_Y410 => &[DXGI_FORMAT_R10G10B10A2_UNORM],
        DXGI_FORMAT_Y416 => &[DXGI_FORMAT_R16G16B16A16_UNORM],
        _ => &[],
    }
}

/// Verifies creating an NT-handle shared texture pair and the presenter's slice SRVs on `render_device`.
pub(super) fn preflight_format_and_srv(
    decode_device: &ID3D11Device,
    render_device: &ID3D11Device,
    render_device1: &ID3D11Device1,
    format: DXGI_FORMAT,
) -> Result<(), String> {
    let (_dec, ren) = create_shared_texture_pair(decode_device, render_device1, format, 64, 64)?;
    for &view_format in srv_formats_for(format) {
        let mut view = None;
        let desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
            Format: view_format,
            ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2DARRAY,
            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2DArray: D3D11_TEX2D_ARRAY_SRV {
                    MostDetailedMip: 0,
                    MipLevels: 1,
                    FirstArraySlice: 0,
                    ArraySize: 1,
                },
            },
        };
        // SAFETY: `ren` is a live shared texture created on `render_device` with `SHADER_RESOURCE`.
        unsafe { render_device.CreateShaderResourceView(&ren, Some(&desc), Some(&mut view)) }
            .map_err(|e| {
                format!(
                    "creating SRV format {} on shared format {}: {e}",
                    view_format.0, format.0
                )
            })?;
    }
    Ok(())
}
